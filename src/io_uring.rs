use crate::TCP_SERVER_PORT;
use bytesize::ByteSize;
use simplelog::*;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use tokio::time::{sleep, timeout};
use tokio_uring::buf::BoundedBuf;
use tokio_uring::buf::BoundedBufMut;
use tokio_uring::fs::File;
use tokio_uring::fs::OpenOptions;
use tokio_uring::net::TcpListener;
use tokio_uring::net::TcpStream;
use tokio_uring::BufResult;
use tokio_uring::UnsubmittedWrite;

// module name for logging engine
const NAME: &str = "<i><bright-black> proxy: </>";

const USB_ACCESSORY_PATH: &str = "/dev/usb_accessory";
const BUFFER_LEN: usize = 16 * 1024;
const READ_TIMEOUT: Duration = Duration::new(5, 0);
const TCP_CLIENT_TIMEOUT: Duration = Duration::new(30, 0);

// tokio_uring::fs::File and tokio_uring::net::TcpStream are using different
// read and write calls:
// File is using read_at() and write_at(),
// TcpStream is using read() and write()
//
// In our case we are reading a special unix character device for
// the USB gadget, which is not a regular file where an offset is important.
// We just use offset 0 for reading and writing, so below is a trait
// for this, to be able to use it in a generic copy() function below.

pub trait Endpoint<E> {
    async fn read<T: BoundedBufMut>(&self, buf: T) -> BufResult<usize, T>;
    fn write<T: BoundedBuf>(&self, buf: T) -> UnsubmittedWrite<T>;
}

impl Endpoint<File> for File {
    async fn read<T: BoundedBufMut>(&self, buf: T) -> BufResult<usize, T> {
        self.read_at(buf, 0).await
    }
    fn write<T: BoundedBuf>(&self, buf: T) -> UnsubmittedWrite<T> {
        self.write_at(buf, 0)
    }
}

impl Endpoint<TcpStream> for TcpStream {
    async fn read<T: BoundedBufMut>(&self, buf: T) -> BufResult<usize, T> {
        self.read(buf).await
    }
    fn write<T: BoundedBuf>(&self, buf: T) -> UnsubmittedWrite<T> {
        self.write(buf)
    }
}

async fn copy<A: Endpoint<A>, B: Endpoint<B>>(
    from: Rc<A>,
    to: Rc<B>,
    dbg_name: &'static str,
    bytes_written: Arc<AtomicUsize>,
) -> Result<(), std::io::Error> {
    let mut buf = vec![0u8; BUFFER_LEN];
    loop {
        // things look weird: we pass ownership of the buffer to `read`, and we get
        // it back, _even if there was an error_. There's a whole trait for that,
        // which `Vec<u8>` implements!
        debug!("{}: before read", dbg_name);
        let retval = from.read(buf);
        let (res, buf_read) = timeout(READ_TIMEOUT, retval).await?;
        // Propagate errors, see how many bytes we read
        let n = res?;
        debug!("{}: after read, {} bytes", dbg_name, n);
        if n == 0 {
            // A read of size zero signals EOF (end of file), finish gracefully
            return Ok(());
        }

        // The `slice` method here is implemented in an extension trait: it
        // returns an owned slice of our `Vec<u8>`, which we later turn back
        // into the full `Vec<u8>`
        debug!("{}: before write", dbg_name);
        let (res, buf_write) = to.write(buf_read.slice(..n)).submit().await;
        let n = res?;
        debug!("{}: after write, {} bytes", dbg_name, n);
        // Increment byte counters for statistics
        bytes_written.fetch_add(n, Ordering::Relaxed);

        // Later is now, we want our full buffer back.
        // That's why we declared our binding `mut` way back at the start of `copy`,
        // even though we moved it into the very first `TcpStream::read` call.
        buf = buf_write.into_inner();
    }
}

async fn transfer_monitor(
    stats_interval: Option<Duration>,
    usb_bytes_written: Arc<AtomicUsize>,
    tcp_bytes_written: Arc<AtomicUsize>,
) -> Result<(), std::io::Error> {
    let mut usb_bytes_out_last: usize = 0;
    let mut tcp_bytes_out_last: usize = 0;
    let mut report_time = Instant::now();

    loop {
        // Stats printing
        if stats_interval.is_some() && report_time.elapsed() > stats_interval.unwrap() {
            // load current total transfer from AtomicUsize:
            let usb_bytes_out = usb_bytes_written.load(Ordering::Relaxed);
            let tcp_bytes_out = tcp_bytes_written.load(Ordering::Relaxed);

            // compute USB transfer
            usb_bytes_out_last = usb_bytes_out - usb_bytes_out_last;
            let usb_transferred_total = ByteSize::b(usb_bytes_out.try_into().unwrap());
            let usb_transferred_last = ByteSize::b(usb_bytes_out_last.try_into().unwrap());
            let usb_speed: u64 =
                (usb_bytes_out_last as f64 / report_time.elapsed().as_secs_f64()).round() as u64;
            let usb_speed = ByteSize::b(usb_speed);

            // compute TCP transfer
            tcp_bytes_out_last = tcp_bytes_out - tcp_bytes_out_last;
            let tcp_transferred_total = ByteSize::b(tcp_bytes_out.try_into().unwrap());
            let tcp_transferred_last = ByteSize::b(tcp_bytes_out_last.try_into().unwrap());
            let tcp_speed: u64 =
                (tcp_bytes_out_last as f64 / report_time.elapsed().as_secs_f64()).round() as u64;
            let tcp_speed = ByteSize::b(tcp_speed);

            info!(
                "{} {} {: >9} ({: >9}/s), {: >9} total | {} {: >9} ({: >9}/s), {: >9} total",
                NAME,
                "phone -> car 🔺",
                usb_transferred_last.to_string_as(true),
                usb_speed.to_string_as(true),
                usb_transferred_total.to_string_as(true),
                "car -> phone 🔻",
                tcp_transferred_last.to_string_as(true),
                tcp_speed.to_string_as(true),
                tcp_transferred_total.to_string_as(true),
            );

            // save values for next iteration
            report_time = Instant::now();
            usb_bytes_out_last = usb_bytes_out;
            tcp_bytes_out_last = tcp_bytes_out;
        }

        sleep(Duration::from_millis(100)).await;
    }
}

pub async fn io_loop(
    stats_interval: Option<Duration>,
    need_restart: Arc<Notify>,
    tcp_start: Arc<Notify>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("{} 🛰️ Starting TCP server...", NAME);
    let bind_addr = format!("0.0.0.0:{}", TCP_SERVER_PORT).parse().unwrap();
    let listener = TcpListener::bind(bind_addr).unwrap();
    info!("{} 🛰️ TCP server bound to: <u>{}</u>", NAME, bind_addr);
    loop {
        info!("{} 💤 waiting for bluetooth handshake...", NAME);
        tcp_start.notified().await;

        // Asynchronously wait for an inbound TCP connection
        info!("{} 🛰️ TCP server: listening for phone connection...", NAME);
        let retval = listener.accept();
        let (stream, addr) = match timeout(TCP_CLIENT_TIMEOUT, retval)
            .await
            .map_err(|e| std::io::Error::other(e))
        {
            Ok(Ok((stream, addr))) => (stream, addr),
            Err(e) | Ok(Err(e)) => {
                error!("{} 📵 TCP server: {}, restarting...", NAME, e);
                // notify main loop to restart
                need_restart.notify_one();
                continue;
            }
        };
        info!(
            "{} 📳 TCP server: new client connected: <b>{:?}</b>",
            NAME, addr
        );

        info!(
            "{} 📂 Opening USB accessory device: <u>{}</u>",
            NAME, USB_ACCESSORY_PATH
        );
        let usb = OpenOptions::new()
            .read(true)
            .write(true)
            .create(false)
            .open(USB_ACCESSORY_PATH)
            .await?;

        info!("{} ♾️ Starting to proxy data between TCP and USB...", NAME);

        // `read` and `write` take owned buffers (more on that later), and
        // there's no "per-socket" buffer, so they actually take `&self`.
        // which means we don't need to split them into a read half and a
        // write half like we'd normally do with "regular tokio". Instead,
        // we can send a reference-counted version of it. also, since a
        // tokio-uring runtime is single-threaded, we can use `Rc` instead of
        // `Arc`.
        let file = Rc::new(usb);
        let file_bytes = Arc::new(AtomicUsize::new(0));
        let stream = Rc::new(stream);
        let stream_bytes = Arc::new(AtomicUsize::new(0));

        // We need to copy in both directions...
        let mut from_file = tokio_uring::spawn(copy(
            file.clone(),
            stream.clone(),
            "USB",
            stream_bytes.clone(),
        ));
        let mut from_stream = tokio_uring::spawn(copy(
            stream.clone(),
            file.clone(),
            "TCP",
            file_bytes.clone(),
        ));

        // Thread for monitoring transfer
        let mut monitor =
            tokio_uring::spawn(transfer_monitor(stats_interval, file_bytes, stream_bytes));

        // Stop as soon as one of them errors
        let res = tokio::try_join!(&mut from_file, &mut from_stream, &mut monitor);
        if let Err(e) = res {
            error!("{} Connection error: {}", NAME, e);
        }
        // Make sure the reference count drops to zero and the socket is
        // freed by aborting both tasks (which both hold a `Rc<TcpStream>`
        // for each direction)
        from_file.abort();
        from_stream.abort();
        monitor.abort();

        // stream(s) closed, notify main loop to restart
        need_restart.notify_one();
    }
}

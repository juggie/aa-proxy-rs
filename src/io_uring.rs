use crate::TCP_SERVER_PORT;
use bytesize::ByteSize;
use humantime::format_duration;
use simplelog::*;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{mpsc, Notify};
use tokio::task::JoinHandle;
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

// Just a generic Result type to ease error handling for us. Errors in multithreaded
// async contexts needs some extra restrictions
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

const USB_ACCESSORY_PATH: &str = "/dev/usb_accessory";
pub const BUFFER_LEN: usize = 16 * 1024;
const TCP_CLIENT_TIMEOUT: Duration = Duration::new(30, 0);

use crate::mitm::endpoint_reader;
use crate::mitm::proxy;
use crate::mitm::Packet;
use crate::mitm::ProxyType;
use crate::usb_stream;
use crate::usb_stream::{UsbStreamRead, UsbStreamWrite};
use crate::HexdumpLevel;

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

pub enum IoDevice<A: Endpoint<A>> {
    UsbReader(Rc<RefCell<UsbStreamRead>>, PhantomData<A>),
    UsbWriter(Rc<RefCell<UsbStreamWrite>>, PhantomData<A>),
    EndpointIo(Rc<A>),
    TcpStreamIo(Rc<TcpStream>),
}

async fn transfer_monitor(
    stats_interval: Option<Duration>,
    usb_bytes_written: Arc<AtomicUsize>,
    tcp_bytes_written: Arc<AtomicUsize>,
    read_timeout: Duration,
) -> Result<()> {
    let mut usb_bytes_out_last: usize = 0;
    let mut tcp_bytes_out_last: usize = 0;
    let mut stall_usb_bytes_last: usize = 0;
    let mut stall_tcp_bytes_last: usize = 0;
    let mut report_time = Instant::now();
    let mut stall_check = Instant::now();

    loop {
        // load current total transfer from AtomicUsize:
        let usb_bytes_out = usb_bytes_written.load(Ordering::Relaxed);
        let tcp_bytes_out = tcp_bytes_written.load(Ordering::Relaxed);

        // Stats printing
        if stats_interval.is_some() && report_time.elapsed() > stats_interval.unwrap() {
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

        // transfer stall detection
        if stall_check.elapsed() > read_timeout {
            // compute delta since last check
            stall_usb_bytes_last = usb_bytes_out - stall_usb_bytes_last;
            stall_tcp_bytes_last = tcp_bytes_out - stall_tcp_bytes_last;

            if stall_usb_bytes_last == 0 || stall_tcp_bytes_last == 0 {
                return Err("unexpected transfer stall".into());
            }

            // save values for next iteration
            stall_check = Instant::now();
            stall_usb_bytes_last = usb_bytes_out;
            stall_tcp_bytes_last = tcp_bytes_out;
        }

        sleep(Duration::from_millis(100)).await;
    }
}

async fn flatten<T>(handle: &mut JoinHandle<Result<T>>) -> Result<T> {
    match handle.await {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(err),
        Err(_) => Err("handling failed".into()),
    }
}

/// Asynchronously wait for an inbound TCP connection
/// returning TcpStream of first client connected
async fn tcp_wait_for_connection(listener: &mut TcpListener) -> Result<TcpStream> {
    let retval = listener.accept();
    let (stream, addr) = match timeout(TCP_CLIENT_TIMEOUT, retval)
        .await
        .map_err(|e| std::io::Error::other(e))
    {
        Ok(Ok((stream, addr))) => (stream, addr),
        Err(e) | Ok(Err(e)) => {
            error!("{} 📵 TCP server: {}, restarting...", NAME, e);
            return Err(Box::new(e));
        }
    };
    info!(
        "{} 📳 TCP server: new client connected: <b>{:?}</b>",
        NAME, addr
    );
    // disable Nagle algorithm, so segments are always sent as soon as possible,
    // even if there is only a small amount of data
    stream.set_nodelay(true)?;

    Ok(stream)
}

pub async fn io_loop(
    stats_interval: Option<Duration>,
    need_restart: Arc<Notify>,
    tcp_start: Arc<Notify>,
    read_timeout: Duration,
    mitm: bool,
    dpi: Option<u16>,
    developer_mode: bool,
    disable_media_sink: bool,
    disable_tts_sink: bool,
    remove_tap_restriction: bool,
    video_in_motion: bool,
    hex_requested: HexdumpLevel,
    wired: bool,
    dhu: bool,
) -> Result<()> {
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
        // disable Nagle algorithm, so segments are always sent as soon as possible,
        // even if there is only a small amount of data
        stream.set_nodelay(true)?;

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

        info!("{} ♾️ Starting to proxy data between HU and MD...", NAME);
        let started = Instant::now();

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

        let mut from_file;
        let mut from_stream;
        let mut reader_hu;
        let mut reader_md;

        // MITM/proxy mpsc channels:
        let (tx_hu, rx_md): (Sender<Packet>, Receiver<Packet>) = mpsc::channel(10);
        let (tx_md, rx_hu): (Sender<Packet>, Receiver<Packet>) = mpsc::channel(10);
        let (txr_hu, rxr_md): (Sender<Packet>, Receiver<Packet>) = mpsc::channel(10);
        let (txr_md, rxr_hu): (Sender<Packet>, Receiver<Packet>) = mpsc::channel(10);

        // dedicated reading threads:
        reader_hu = tokio_uring::spawn(endpoint_reader(file.clone(), txr_hu));
        reader_md = tokio_uring::spawn(endpoint_reader(stream.clone(), txr_md));
        // main processing threads:
        from_file = tokio_uring::spawn(proxy(
            ProxyType::HeadUnit,
            file.clone(),
            stream_bytes.clone(),
            tx_hu,
            rx_hu,
            rxr_md,
            dpi,
            developer_mode,
            disable_media_sink,
            disable_tts_sink,
            remove_tap_restriction,
            video_in_motion,
            !mitm,
            hex_requested,
        ));
        from_stream = tokio_uring::spawn(proxy(
            ProxyType::MobileDevice,
            stream.clone(),
            file_bytes.clone(),
            tx_md,
            rx_md,
            rxr_hu,
            dpi,
            developer_mode,
            disable_media_sink,
            disable_tts_sink,
            remove_tap_restriction,
            video_in_motion,
            !mitm,
            hex_requested,
        ));

        // Thread for monitoring transfer
        let mut monitor = tokio::spawn(transfer_monitor(
            stats_interval,
            file_bytes,
            stream_bytes,
            read_timeout,
        ));

        // Stop as soon as one of them errors
        let res = tokio::try_join!(
            flatten(&mut reader_hu),
            flatten(&mut reader_md),
            flatten(&mut from_file),
            flatten(&mut from_stream),
            flatten(&mut monitor)
        );
        if let Err(e) = res {
            error!("{} 🔴 Connection error: {}", NAME, e);
        }
        // Make sure the reference count drops to zero and the socket is
        // freed by aborting both tasks (which both hold a `Rc<TcpStream>`
        // for each direction)
        reader_hu.abort();
        reader_md.abort();
        from_file.abort();
        from_stream.abort();
        monitor.abort();

        info!(
            "{} ⌛ session time: {}",
            NAME,
            format_duration(started.elapsed()).to_string()
        );
        // stream(s) closed, notify main loop to restart
        need_restart.notify_one();
    }
}

# 🛸 aa-proxy-rs

## About
This is a Rust-Written proxy tool to bridge between wireless android phone and a USB-wired car head unit for using Google's Android Auto.
Currently it is intended to run as a more-or-less drop-in replacement of the `aawgd` from the [WirelessAndroidAutoDongle](https://github.com/nisargjhaveri/WirelessAndroidAutoDongle) project.

## History and motivation
There are a lot of commercial solutions like AAWireless or Motorola MA1. I even bought a clone of those on AliExpress, but it ended up not working in my car (passed to my friend which has a compatible car for this).

Then thanks to [Nicnl](https://www.reddit.com/user/Nicnl/) from reddit under my [post](https://www.reddit.com/r/RenaultZoe/comments/1c5eg2g/working_wireless_aa_for_rlink1_based_zoe/),
I headed to a great open source solution, based on Raspberry Pi hardware:<br>
[WirelessAndroidAutoDongle](https://github.com/nisargjhaveri/WirelessAndroidAutoDongle) by [Nisarg Jhaveri](https://github.com/nisargjhaveri)

The author made a lot of great research and created a DIY working solution, and - what is most important - he shared his work to the public.
This is so cool and I really appreciate his work!
Because it's open source, I was even able to run my own additional LoRa hardware on the same Raspberry Pi for different purpose!

The original project is using `aawgd` daemon which is doing the necessary proxying of data between the phone and USB port.
(Un)fortunately the project was not always working reliable for me (crashing, unable to reconnect, need restarting, etc.).
Finally after making this [PR](https://github.com/nisargjhaveri/WirelessAndroidAutoDongle/pull/196), I decided that I will try to make the Rust-based alternative.
And this is where this project begins and I started to reimplement the `aawgd` C++ code into rust application.

## Coding
I was trying to simplify things and original code where it was possible.
It was also a great opportunity and fun to learn how it was designed and how it is working, but I have to admit that I was struggling a lot with doing a final forwarding I/O:
normally I would just use [copy_bidirectional](https://docs.rs/tokio/latest/tokio/io/fn.copy_bidirectional.html) for this as the whole code is async and using [tokio](https://tokio.rs/), but
the problem with it was, that the USB socket for the "usb-gadget" kernel module seems to be not compatible with this approach (probably some polling/epoll problems).
I was also trying to call read/writes in tokio tasks, but finally I decided to use different approach: using modern [io_uring](https://kernel.dk/io_uring.pdf) kernel API provided by [tokio_uring](https://github.com/tokio-rs/tokio-uring).
And this finally worked perfectly fine (and also really efficient as a bonus).

## Limitations
Currently only the default "connection strategy" is supported. I don't have a clue if and when I add the other ones. Time will tell.
My time resources are limited, so don't expect to prompt answers and ETAs on different requests. I am doing this as a hobby in my spare time.
I've tested this only with my own `Raspberry Pi Zero 2 W` and my specific car head unit (old Renault R-Link).
Config parameters from `/etc/aawgd.env` are not (yet?) supported.

## Current stage and plans
This project is on early stage of development:
The tool is currently working fine for me from Raspberry Pi boot up to initial phone connection. It is then working stable until the phone goes out of range.
There is left a lot of work to make it more stable and reliable, especially I am planning to add reconnecting/recovering code where it is applicable.

## How it works (technical)
![Hardware overview](images/aa-proxy-rs.webp)
The whole connection process is not trivial and quite complex. Here I am listing the needed steps the app is doing from the start to make a connection:
- USB: disabling all gadgets
- USB: registering uevents (for receiving USB state changes)
- Starting local TCP server
- Bluetooth: powering up the bluetooth adapter and make it discoverable and pairable
- Bluetooth: registering two profiles: one for android auto, and the other for fake headset (fooling the phone we are supported wireless AndroidAuto head unit)
- When a phone connects to the AA profile the app is sending two frames of specific google protocol data:
  - WifiStartRequest: with IP address and port of the destination TCP server/connection
  - WifiInfoResponse: with Access Point information for the WiFi connection
- after successfull response for the above, the tool is disabling bluetooth
- the phone is connecting to car's head unit bluetooth (e.g. for phone calls)
- in the same time the phone is connecting via WiFi to our TCP server on specified port
- USB: switching to "default" followed by "accessory" gadgets to enable proper USB mode for data transmission to the car head unit (fooling that we are the android phone connected via USB)
- final (and normal working stage): bidirectional forwarding the data between TCP client (phone) and USB port (car)

USB is the active part here (starting the transmission by sending 10-bytes first frame), so all of this has to be done with well timing, i.e., if we start the USB dongle connection too fast
(when the phone is not yet connected), then it would start the transmission, and we don't have a TCP socket ready; similar in the other direction: when the phone starts too fast, and we are
not ready with USB connection then we can't send data to the phone and Android will close/timeout the connection.

## Demo
[![asciicast](https://asciinema.org/a/686949.svg)](https://asciinema.org/a/686949)

## Building and installing
`rpi02w` binaries build by [WirelessAndroidAutoDongle](https://github.com/nisargjhaveri/WirelessAndroidAutoDongle) are for `arm-unknown-linux-gnueabihf` 32-bit architecture, probably
because of usb-gadget module [incompatibility](https://github.com/nisargjhaveri/WirelessAndroidAutoDongle/pull/129).
To be able to properly crosscompile output binary I provided `.cargo/config.toml` with target set for this specific arch.

To compile you need to add proper rustup target with:
```
rustup target add arm-unknown-linux-gnueabi
```
and make sure that it is _installed_ on target list:
```
arm-unknown-linux-gnueabihf (installed)
```
Besides a `binutils-arm-linux-gnueabihf` package is needed on Debian. This is distro-depended so I recommend to RTFM.

After building you need to transfer the binary to the target filesystem (I am using ssh/scp for this) and start it.
For permanent solution I also modified startup scripts - but how to do it is out of scope of this document.

## Usage
```
aa-proxy-rs 0.1.0
AndroidAuto wired/wireless proxy

USAGE:
    aa-proxy-rs [OPTIONS]

OPTIONS:
    -d, --debug                       Enable debug info
    -h, --help                        Print help information
    -l, --logfile <LOGFILE>           Log file path [default: /var/log/aa-proxy-rs.log]
    -s, --stats-interval <SECONDS>    Interval of showing data transfer statistics (0 = disabled)
                                      [default: 0]
    -V, --version                     Print version information
```

## Troubleshooting
Sometimes deleting the system Bluetooth cache at /var/lib/bluetooth and restarting bluetoothd fixes persistent issues with device connectivity.
Consider also using "Forget" of bluetooth device in the Android phone.

## Known problems
During my development work I encountered the stuck USB adapter once. What is more interesting, a reboots doesn't help, I had to re-power cycle the Pi.
In the `dmesg` I've got this:
```
Jan  1 00:00:02 buildroot kern.info kernel: [    1.775627] Bluetooth: hci0: BCM: chip id 94
Jan  1 00:00:02 buildroot kern.info kernel: [    1.779290] Bluetooth: hci0: BCM: features 0x2e
Jan  1 00:00:02 buildroot kern.info kernel: [    1.793222] Bluetooth: hci0: BCM43430A1
Jan  1 00:00:02 buildroot kern.info kernel: [    1.795614] Bluetooth: hci0: BCM43430A1 (001.002.009) build 0000
Jan  1 00:00:02 buildroot kern.info kernel: [    1.801062] Bluetooth: hci0: BCM43430A1 'brcm/BCM43430A1.raspberrypi,model-zero-2-w.hcd' Patch
Jan  1 00:00:04 buildroot kern.info kernel: [    4.436283] Bluetooth: hci0: BCM: features 0x2e
Jan  1 00:00:04 buildroot kern.err  kernel: [    4.438581] Bluetooth: hci0: Frame reassembly failed (-84)
Jan  1 00:00:04 buildroot kern.warn kernel: [    4.438627] Bluetooth: hci0: Received unexpected HCI Event 0x00
Jan  1 00:00:04 buildroot kern.err  kernel: [    4.440329] Bluetooth: hci0: Frame reassembly failed (-84)
Jan  1 00:00:06 buildroot kern.err  kernel: [    6.473578] Bluetooth: hci0: command 0x0c14 tx timeout
Jan  1 00:00:14 buildroot kern.err  kernel: [   14.553559] Bluetooth: hci0: BCM: Reading local name failed (-110)
```
And this was leading to problem with `aawgd`:
```
Jan  1 00:00:04 buildroot user.info aawgd[237]: Did not find any bluetooth adapters
```
As well as in aa-proxy-rs which cannot find bluetooth adapter.
I didn't go further investigating this problem, only noticing.

## Similar projects
- https://github.com/nisargjhaveri/WirelessAndroidAutoDongle
- https://github.com/nisargjhaveri/AAWirelessGateway
- https://github.com/openDsh/openauto
- https://github.com/qhuyduong/AAGateway
- https://github.com/Demon000/web-auto

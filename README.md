# 🛸 aa-proxy-rs

[![Discord](https://img.shields.io/discord/1358047273434615968?style=for-the-badge&logo=discord&logoColor=white&label=Discord&labelColor=%235865F2&color=%231825A2)](https://discord.gg/c7JKdwHyZu)

## About
This is a Rust-Written proxy tool to bridge between wireless Android phone and a USB-wired car head unit for using Google's Android Auto.
Currently it is intended to run as a more-or-less drop-in replacement of the `aawgd` from the [WirelessAndroidAutoDongle](https://github.com/nisargjhaveri/WirelessAndroidAutoDongle) project.

## Features
- written in [Rust](https://www.rust-lang.org/): reliable code and minimized all memory related issues and bugs
- fast: main IO loop is using modern [io_uring](https://kernel.dk/io_uring.pdf) kernel API
- reconnecting: trying to reconnect/recover AndroidAuto connection in any possible case
- bandwidth/transfer statistics
- [embedded web interface](#embedded-web-interface)
- stall transfer detection
- [MITM (man-in-the-middle) mode](#mitm-mode) support with the following features:
  - DPI change
  - Remove tap restriction
  - Disable media sink
  - Disable TTS sink
  - Video in motion
  - Developer mode
- [Google Maps EV Routing](#google-maps-ev-routing) support
- Wired phone USB connection mode (without Bluetooth handshake and WiFi)
- Google `Desktop Head Unit` emulator support for debugging purposes

## Current project status
Now after a lot of stress-testing and coding I think the project has matured enough, that I can say that the main stability goal was reached.
I am using this project almost daily in my car and trying to get rid of all issues I may encounter.<br>
There is also great and helpful community on Discord around this project. If you want to join or ask for help/support then feel free to connect to our server: [aa-proxy-rs](https://discord.gg/c7JKdwHyZu).

## SD card images
I am using Nisarg's RaspberryPi images from [WirelessAndroidAutoDongle](https://github.com/nisargjhaveri/WirelessAndroidAutoDongle) project and replacing `aawgd` with `aa-proxy-rs`.<br>
Those images are available on the [Release page](https://github.com/manio/aa-proxy-rs/releases).<br>
You can also find there a pure `aa-proxy-rs` binary which you can install manually on targets with similar libc versions, or build your own (read below: [Installing into target](#installing-into-target) and [Building](#building)).

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
- Currently only the default "connection strategy" is supported.
- Because the project main functionality (data transfer) is dependent on kernel [io_uring](https://kernel.dk/io_uring.pdf) API, the Linux kernel has to be in version 5.10 or later.
- My time resources are limited, so don't expect to prompt answers and ETAs on different requests. I am doing this as a hobby in my spare time.

## Embedded web interface
When you connect to the device's WiFi network, you can access the web interface, which is available by default at: [http://10.0.0.1](http://10.0.0.1).<br>
Using the web interface, you can configure all settings that are also available in `/etc/aa-proxy-rs/config.toml`:<br>
![Webserver preview](images/webserver.png)
You can also download logs with a single click.

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

## Building
`rpi02w` binaries build by [WirelessAndroidAutoDongle](https://github.com/nisargjhaveri/WirelessAndroidAutoDongle) are for `arm-unknown-linux-gnueabihf` 32-bit architecture, probably
because of usb-gadget module [incompatibility](https://github.com/nisargjhaveri/WirelessAndroidAutoDongle/pull/129).
To be able to properly crosscompile output binary I provided `.cargo/config.toml` with target set for this specific arch.

### Dependencies
1. [Installing Rust](https://www.rust-lang.org/tools/install)
2. `gcc-arm-linux-gnueabihf` package is needed on Debian. This is distro-depended so I recommend to RTFM.

To compile you need to add proper rustup target with:
```
rustup target add arm-unknown-linux-gnueabihf
```
and make sure that it is _installed_ on target list:
```
arm-unknown-linux-gnueabihf (installed)
```
and then use:
```
cargo build --release
```

To compile a STATIC `aa-proxy-rs` binary (to be able to use it on target with older OSes and different libc versions), you need to compile with:
```
RUSTFLAGS='-C target-feature=+crt-static' cargo build --release
```

## Building using Docker
To build with Docker you need to have a [buildx](https://github.com/docker/buildx) and [BuildKit](https://github.com/moby/buildkit).<br>
Docker container is also preparing an SD card images based on [@nisargjhaveri](https://github.com/nisargjhaveri)'s [latests assets](https://github.com/nisargjhaveri/WirelessAndroidAutoDongle/releases).
It has to loop-mount that images, thus an insecure builder is [needed](https://docs.docker.com/reference/cli/docker/buildx/build/#allow).
To sum it up - the following commands are needed when building for the first time:
```
mkdir out
docker buildx create --use --name insecure-builder --buildkitd-flags '--allow-insecure-entitlement security.insecure'
docker buildx build --builder insecure-builder --allow security.insecure --output out .
```
After successful execution the resulting `aa-proxy-rs` and SD card images will be in `out` directory.

## Installing into target
If you currently using a Raspberry Pi with working _WirelessAndroidAutoDongle_, then you can also manually install `aa-proxy-rs`:

You need to transfer the resulting output binary to the target filesystem and start it. I am using ssh/scp for this, but it should be also possible with `wget`.
You can also do it "offline" by making a changes directly on the SD card: mounting system partition and make necessary changes.
Sample [startup script](https://raw.githubusercontent.com/manio/aa-proxy-rs/refs/heads/main/contrib/S93aa-proxy-rs) is provided for convenience.

Example steps:
- put `aa-proxy-rs` into /usr/bin
- put `S93aa-proxy-rs` into /etc/init.d
- remove or disable /etc/init.d/S93aawgd

Startup parameters (see below) are defined [here](https://github.com/manio/aa-proxy-rs/blob/main/contrib/S93aa-proxy-rs#L10).

## Usage
```
Usage: aa-proxy-rs [OPTIONS]

Options:
  -c, --config <CONFIG>  Config file path [default: /etc/aa-proxy-rs/config.toml]
  -h, --help             Print help
  -V, --version          Print version
```

## Configuration
Default startup config file is [config.toml](https://github.com/manio/aa-proxy-rs/blob/main/contrib/config.toml).

Configuration options are documented in comments, but these needs some more attention:<br>
- `legacy`<br>
Original `aawgd` is using two USB gadgets: **default** and **accessory**. When connecting to car headunit, it switches first to **default** then to **accessory**.
During my development I found out that my car headunit doesn't need this switching. It is working fine connecting directly to **accessory** gadget.
Moreover with this approach it is much faster and doesn't need to wait for USB events in dedicated _UEvent_ thread. As the result I decided to leave the old (legacy)
code under this switch for compatibility with some headunits.<br>
In short: if you have problems with USB connection try to enable the legacy mode.

- `connect`<br>
By default without this option the aa-proxy-rs is starting but it is only visible as a bluetooth dongle, to which you have to connect manually from your phone to
initiate AndroidAuto connection. If I am correct this was called `dongle mode` in `aawgd`.<br>
If you provide `connect` option with default `00:00:00:00:00:00` wildcard address, then the daemon is trying to connect to known (paired?) bluetooth devices (phones) in a loop
(the **bluetoothd** have a cached list of recently connected devices in /var/lib/bluetooth). This is the default mode for `aawgd` for the time I am writing this.<br>
If you set this option to specific `MAC_ADDRESS` where MAC_ADDRESS is the MAC of your phone (bluetooth), then the aa-proxy-rs will try to connect only to this specified device
in a loop (ignoring all **bluetoothd** cached devices).

## MITM mode
Man-in-the-middle mode support has been added recently. This is the mode which allows to change the data passed between the HU and the phone.
Separate encrypted connections are made to each device to be able to see or modify the data passed between HU and MD.<br>
This is opening new possibilities like, e.g., forcing HU to specific DPI, adding EV capabilities to HU/cars which doesn't support this Google Maps feature.<br>
All the above is not currently supported but should be possible and easier with this mode now implemented.<br>
To have this mode working you need enable `mitm` option in configuration and provide certificate and private key for communication for both ends/devices.
Default directory where the keys are search for is: `/etc/aa-proxy-rs/`, and the following file set needs to be there:<br>
- hu_key.pem
- hu_cert.pem
- md_key.pem
- md_cert.pem
- galroot_cert.pem

I will not add these files into this repository to avoid potential problems. You can find it in other places, or even other git repos, like:<br>
- https://github.com/tomasz-grobelny/AACS/tree/master/AAServer/ssl
- https://github.com/tomasz-grobelny/AACS/tree/master/AAClient/ssl
- https://github.com/lucalewin/vehiculum/tree/main/src/server/cert
- https://github.com/lucalewin/vehiculum/tree/main/src/client/cert
- https://github.com/borconi/headunit/blob/master/jni/hu_ssl.h#L29

Special thanks to [@gamelaster](https://github.com/gamelaster/) for the help, support and his [OpenGAL Proxy](https://github.com/gamelaster/opengal_proxy) project.

### DPI settings
Thanks to above MITM mode a DPI setting of the car HU can be forced/replaced. This way we can change the hardcoded value to our own. This is allowing to view more data (at cost of readability/font size).<br>
Example with Google Maps, where a `Report` button is available after changing this value:

|160 DPI (default)|130 DPI|
|---|---|
|![](images/160dpi.png)|![](images/130dpi.png)

## Google Maps EV routing
Google introduced EV routing features at [CES24](https://blog.google/products/android/android-auto-new-features-ces24/). The first cars to support this via Android Auto are the Ford Mustang Mach-E and F-150 Lightning.

This clip shows how it works in the car:<br>
[![This is a clip how it works in the car](https://img.youtube.com/vi/M1qf9Psu6g8/hqdefault.jpg)](https://www.youtube.com/embed/M1qf9Psu6g8)

The idea of using this feature with other cars started here: https://github.com/manio/aa-proxy-rs/issues/19 in February 2025.
After a long journey searching for someone with the knowledge and hardware that could help us obtain the logs, we finally, at the end of June 2025, thanks to [@SquidBytes](https://github.com/SquidBytes), got the sample data to analyze.

Thanks to many hours of work by [@Deadknight](https://github.com/Deadknight) and [@gamelaster](https://github.com/gamelaster), we were finally able to make some use of that data.
Unfortunately, the work is still in progress, but I am currently at a stage where, by customizing some parameters, I can provide real-time battery level data to `aa-proxy-rs`, and overall it makes correct estimates for my car.

`aa-proxy-rs` has an embedded REST server for obtaining battery data from any source (I am using a slightly modified version of the [canze-rs](https://github.com/manio/canze-rs) app for this purpose).
It reads the data on the same Raspberry Pi (connecting wirelessly to the Bluetooth OBD dongle).

`aa-proxy-rs` can be configured to execute a specific data collection script when Android Auto starts and needs the battery level data, and also when it stops.
The script can be configured in `config.toml` and is executed with the arguments `start` and `stop` accordingly.

Here's a `curl` example to feed the data:<br>
```bash
curl -X POST http://localhost:3030/battery \
     -H "Content-Type: application/json" \
     -d '{"battery_level": 87.5}'
```

Thanks to the power of open source, even older EVs can now enjoy modern features and a much better navigation experience!

## Troubleshooting
Sometimes deleting the system Bluetooth cache at /var/lib/bluetooth and restarting bluetoothd fixes persistent issues with device connectivity.
Consider also using "Forget" of bluetooth device in the Android phone.

Application by default is logging into _/var/log/aa-proxy-rs.log_ file. This log could be helpful when trying to solve issues.
If you want to get logs out of device, read [here](https://github.com/nisargjhaveri/WirelessAndroidAutoDongle?tab=readme-ov-file#getting-logs) how to obtain it.

## Similar/other open source AndroidAuto-related projects
- https://github.com/nisargjhaveri/WirelessAndroidAutoDongle
- https://github.com/nisargjhaveri/AAWirelessGateway
- https://github.com/openDsh/openauto
- https://github.com/qhuyduong/AAGateway
- https://github.com/Demon000/web-auto
- https://github.com/f1xpl/openauto
- https://github.com/gamelaster/opengal_proxy
- https://github.com/tomasz-grobelny/AACS

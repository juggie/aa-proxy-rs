# syntax=docker/dockerfile:1-labs
FROM rust:latest AS stage-rust
# crosscompile stuff
RUN apt update && apt upgrade -y
RUN apt install -y gcc-arm-linux-gnueabihf
RUN rustup target add arm-unknown-linux-gnueabihf
# cloning and building
WORKDIR /usr/src/app
RUN git clone https://github.com/manio/aa-proxy-rs .
RUN cargo build --release

# injecting aa-proxy-rs into all SD card images
FROM alpine AS stage-sdcards
RUN apk --no-cache add xz
WORKDIR /root
COPY --from=stage-rust /usr/src/app/target/arm-unknown-linux-gnueabihf/release/aa-proxy-rs .
ADD contrib/injector.sh .
ADD contrib/S93aa-proxy-rs .
RUN --security=insecure ./injector.sh

# copy the resulting binary out of container to local dir
FROM scratch AS custom-exporter
COPY --from=stage-rust /usr/src/app/target/arm-unknown-linux-gnueabihf/release/aa-proxy-rs .
COPY --from=stage-sdcards /root/raspberrypi0w-sdcard.img.xz .
COPY --from=stage-sdcards /root/raspberrypi3a-sdcard.img.xz .
COPY --from=stage-sdcards /root/raspberrypi4-sdcard.img.xz .
COPY --from=stage-sdcards /root/raspberrypizero2w-sdcard.img.xz .

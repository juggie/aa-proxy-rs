FROM rust:latest AS stage-rust

# crosscompile stuff
RUN apt update && apt upgrade -y
RUN apt install -y gcc-arm-linux-gnueabihf
RUN rustup target add arm-unknown-linux-gnueabihf

# cloning and building
WORKDIR /usr/src/app
RUN git clone https://github.com/manio/aa-proxy-rs .
RUN cargo build --release

# copy the resulting binary out of container to local dir
FROM scratch AS custom-exporter
COPY --from=stage-rust /usr/src/app/target/arm-unknown-linux-gnueabihf/release/aa-proxy-rs .

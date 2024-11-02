FROM rust:1.69 as builder

RUN apt-get update && \
		apt-get install -y python3 python3-pip
RUN pip3 install --no-cache --upgrade pip setuptools wheel
RUN pip3 install meson==1.1.0 ninja==1.10.2.4

WORKDIR /sfu

COPY ./Cargo.toml ./Cargo.toml
RUN ls ./Cargo.lock && cp ./Cargo.lock ./ || true
COPY ./src ./src

RUN cargo build --release

FROM scratch
# # FROM alpine:3.18.3

# #RUN apk add --no-cache libgcc musl libc6-compat libressl-dev libcrypto1.1
# FROM debian:bullseye-slim

# RUN apt-get update && apt-get install -y --no-install-recommends libgcc1 libstdc++6
# RUN apt-get clean && rm -rf /var/lib/apt/lists/*

COPY --from=builder /sfu/target/release/sfu /sfu
ENTRYPOINT ["/sfu"]
EXPOSE 3000
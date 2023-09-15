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
COPY --from=builder /sfu/target/release/sfu /sfu
ENTRYPOINT ["/sfu"]
EXPOSE 3000
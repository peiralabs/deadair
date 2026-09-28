FROM rust:1.98-alpine AS builder

RUN apk add --no-cache musl-dev
RUN rustup target add x86_64-unknown-linux-musl

WORKDIR /build
COPY . .
RUN cargo build --release --target x86_64-unknown-linux-musl

FROM scratch

# ureq uses rustls, which compiles webpki roots into the binary; no CA bundle is needed.
COPY --from=builder /build/target/x86_64-unknown-linux-musl/release/deadair /deadair
EXPOSE 9113
ENTRYPOINT ["/deadair"]
CMD ["watch"]

ARG RUST_VERSION=1.75.0

FROM rust:${RUST_VERSION}-alpine AS builder
WORKDIR /app
COPY . .

RUN apk add --no-cache musl-dev
RUN \
  --mount=type=cache,target=/app/target/ \
  --mount=type=cache,target=/usr/local/cargo/registry/ \
  cargo build --release && \
    cp ./target/release/vxsky /

FROM alpine:3 AS final
WORKDIR /app
RUN addgroup -S myuser && adduser -S myuser -G myuser
COPY --from=builder /vxsky .
USER myuser
CMD ["./vxsky"]
# API and background worker image. PostgreSQL persists outside the containers.
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev build-base cmake perl
WORKDIR /src
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY crates ./crates
COPY contracts ./contracts
ARG VERSION=dev
RUN HONORUM_VERSION=${VERSION} cargo build --release --locked -p honorum --no-default-features && cp target/release/honorum /honorum

FROM alpine:3.22
RUN apk add --no-cache ca-certificates && adduser -D app && mkdir -p /app/data && chown app /app/data
USER app
WORKDIR /app
COPY --from=build /honorum /app/honorum
COPY config/schools.json /app/config.json
EXPOSE 8080
ENTRYPOINT ["/app/honorum", "-config", "/app/config.json"]

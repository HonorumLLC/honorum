# Standalone image: the API, the background worker, and the embedded web app.
FROM node:24-alpine AS web
WORKDIR /src/apps/web
COPY apps/web/package*.json ./
RUN npm ci
COPY apps/web .
RUN npm run build

FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev build-base cmake perl
WORKDIR /src
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY crates ./crates
COPY contracts ./contracts
COPY --from=web /src/apps/web/dist ./apps/web/dist
ARG VERSION=dev
RUN HONORUM_VERSION=${VERSION} cargo build --release --locked -p honorum && cp target/release/honorum /honorum

FROM alpine:3.22
RUN apk add --no-cache ca-certificates && adduser -D app && mkdir -p /app/data && chown app /app/data
USER app
WORKDIR /app
COPY --from=build /honorum /app/honorum
COPY config/schools.json /app/config.json
VOLUME /app/data
EXPOSE 8080
ENTRYPOINT ["/app/honorum", "-config", "/app/config.json"]

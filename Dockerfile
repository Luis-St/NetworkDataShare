# Builds NetworkDataShare into a small runtime image.
# The frontend is built in its own stage, so the Rust stage does not need Node.js.

FROM node:22-bookworm-slim AS web
WORKDIR /app/web
COPY web/package.json web/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY web/ ./
RUN npm run build

FROM rust:1.93-bookworm AS build
WORKDIR /app
COPY Cargo.toml Cargo.lock build.rs ./
COPY src/ src/
COPY --from=web /app/web/dist web/dist
# Embed the frontend built above instead of running npm again.
ENV NDS_SKIP_WEB_BUILD=1
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin nds \
    && mkdir -p /data && chown nds:nds /data
COPY --from=build /app/target/release/network-data-share /usr/local/bin/network-data-share
USER nds
# The generated certificate lives in /data, mount a volume there to keep it across container updates.
ENV NDS_DATA_DIR=/data
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["network-data-share"]

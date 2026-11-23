FROM rust:1.100-bookworm AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY migrations ./migrations
RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl sqlite3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 finapp \
    && useradd --system --uid 10001 --gid finapp --home-dir /app finapp \
    && mkdir -p /app /data \
    && chown finapp:finapp /app /data
WORKDIR /app
COPY --from=build /build/target/release/finapp /usr/local/bin/finapp
# A changed sw.js byte is what makes browsers install the new shell; stamp the commit.
ARG FINAPP_VERSION=unversioned
COPY --chown=finapp:finapp static ./static
RUN sed -i "s/misfinanzas-shell-v9/misfinanzas-shell-${FINAPP_VERSION}/" static/sw.js
USER 10001:10001
ENV FINAPP_BIND=0.0.0.0:3000 FINAPP_DB_PATH=/data/finapp.sqlite
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl --fail --silent http://127.0.0.1:3000/health >/dev/null || exit 1
CMD ["finapp"]

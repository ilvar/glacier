FROM rust:1.97.1-bookworm AS builder

WORKDIR /src
COPY . .
RUN cargo build --release --locked

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        age \
        ca-certificates \
        ffmpeg \
        par2 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /src/target/release/legacy /usr/local/bin/legacy

WORKDIR /data
VOLUME ["/data"]
EXPOSE 8000

# The archive to use when a request names none. Pinned to the volume so a
# default-archive write lands on mounted storage rather than in a
# container-local home directory that vanishes with the container.
ENV LEGACY_ARCHIVE=/data

ENTRYPOINT ["/usr/local/bin/legacy"]
CMD ["serve", "--host", "0.0.0.0", "--port", "8000"]

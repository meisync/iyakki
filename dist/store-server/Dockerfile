# The IYAKKI private store server: `iyakki-store-server`, one small static binary (about 1 MB) in a FROM scratch image, non-root.
#
# Build from the repository root (needs network for the base images):
#   podman build -f dist/store-server/Dockerfile -t iyakki-store-server:1.0.0 .
# On a cluster without internet, IYAKKI builds the same image itself from the static iyakki-store-server (found, built or
# chosen in the wizard: Settings, Dev cluster, Platform services, Private store): see docs/STORE.md, sections 13 and 13.1.
# The program also takes a leading `store-server` argument, so `iyakki store-server ...` command lines work unchanged.
FROM rust:1.89-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p iyakki-store-server --target-dir /out \
 && mkdir -p /rootfs/data && cp /out/release/iyakki-store-server /rootfs/iyakki && chmod 0555 /rootfs/iyakki && chown -R 10001:10001 /rootfs/data

FROM scratch
COPY --from=build /rootfs/iyakki /iyakki
COPY --from=build --chown=10001:10001 /rootfs/data /data
USER 10001:10001
EXPOSE 8080
VOLUME ["/data"]
ENTRYPOINT ["/iyakki"]
CMD ["store-server", "--data", "/data", "--listen", "0.0.0.0:8080"]

# Multi-stage static musl build: `docker build -f dist/Dockerfile -t iyakki:0.1.0 .`
FROM rust:alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
RUN cargo build --release -p iyakki-core
# the data folder (accounts, settings, the browser front end's certificate) and a scratch folder, owned by the unprivileged user
RUN mkdir -p /data/var/lib/iyakki /data/tmp && chmod 1777 /data/tmp

# Runs unprivileged by default; PID 1 boot logic only triggers when the container's PID is 1
# *and* the process is root-capable, so set the user explicitly.
FROM scratch
COPY --from=build /src/target/release/iyakki /iyakki
COPY --from=build --chown=65532:65532 /data/var /var
COPY --from=build --chown=65532:65532 /data/tmp /tmp
# keep the data folder on a volume:  docker run -v iyakki-data:/var/lib/iyakki ...
VOLUME /var/lib/iyakki
# the desktop in a browser (HTTPS, self-signed unless cert.pem/key.pem are put in /var/lib/iyakki/tls):
#   docker run -p 8443:8443 -e IYAKKI_WEB=1 -v iyakki-data:/var/lib/iyakki iyakki:0.1.0 serve
# first account (no screen to do it on):
#   echo 'a long password' | docker run -i --rm -v iyakki-data:/var/lib/iyakki iyakki:0.1.0 user add admin --role admin --password-stdin
EXPOSE 8443
USER 65532:65532
ENTRYPOINT ["/iyakki"]
CMD ["serve"]

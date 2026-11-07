# The IYAKKI cloud container image. Built by dist/cloud/build-cloud.sh from a small prepared context (never from the repository):
#   iyakki                       the static musl program (built outside, with the repository's release profile)
#   etc/ssl/certs/ca-certificates.crt   the CA bundle (HTTPS to model providers, registries, git hosts)
#   licenses/                    LICENSE, NOTICE, THIRD_PARTY_LICENSES.txt, the notices index and the CycloneDX SBOM
#   etc/iyakki/*.example.json    examples (service profile, tenant configuration)
#   var/lib/iyakki, tmp          empty folders
# FROM scratch: no shell, no package manager, nothing to patch but the program. Non-root, read-only-rootfs friendly (the only writable
# places are the volume /var/lib/iyakki and /tmp). No base image is pulled.
FROM scratch
COPY iyakki /iyakki
COPY etc/ /etc/
COPY licenses/ /licenses/
COPY --chown=65532:65532 var/ /var/
COPY --chown=65532:65532 tmp/ /tmp/
ENV IYAKKI_WEB=1 \
    IYAKKI_ADOPT=1 \
    IYAKKI_LOG=info
VOLUME /var/lib/iyakki
EXPOSE 8443
USER 65532:65532
ENTRYPOINT ["/iyakki"]
# default: the desktop in a browser on https://HOST:8443/. With no account yet it prints a pairing code in the container log (adopt mode).
# Other commands:  serve --profile cloud --config /etc/iyakki/service.json   (headless web service, HTTPS on its own port)
#                  cloud listen --routes /etc/iyakki/routes.json              (router for many tenants)
#                  cloud tenants|manifests|deploy|snapshot ...                (control plane, run where kubectl is)
#                  user add NAME --role admin --password-stdin                (first account without the pairing page)
CMD ["serve"]
# the probe is the program itself: GET /healthz on the local listener (set IYAKKI_HEALTH_URL for another port)
HEALTHCHECK --interval=30s --timeout=6s --start-period=20s --retries=3 CMD ["/iyakki", "healthcheck"]

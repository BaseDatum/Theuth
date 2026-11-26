# Caddy with the Porkbun DNS provider (DNS-01 only; HTTP-01/TLS-ALPN-01 would race the old IP during cutover).
# Built on the host by `docker compose build caddy`; versions pinned in .env.
ARG CADDY_VERSION
FROM caddy:${CADDY_VERSION}-builder AS build
ARG CADDY_PORKBUN_VERSION
RUN xcaddy build --with github.com/caddy-dns/porkbun@${CADDY_PORKBUN_VERSION}

FROM caddy:${CADDY_VERSION}
COPY --from=build /usr/bin/caddy /usr/bin/caddy
# Runs as 65532 (compose), listens on 8080/8443 behind HAProxy: no file capability needed.
RUN setcap -r /usr/bin/caddy 2>/dev/null || true

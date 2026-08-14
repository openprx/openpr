#!/bin/sh
# The proxy_pass targets are written as variables so nginx starts even when the api
# container is not up yet, but that form makes nginx resolve them at request time and
# therefore requires an explicit `resolver`. The address differs per container runtime
# and per host (podman hands out 10.89.0.1 on one machine and 10.89.3.1 on another), so
# hardcoding it makes the image work on exactly one host and hang with a 499 everywhere
# else. Take it from the resolver the runtime already gave this container.
set -eu

resolver_conf=/etc/nginx/conf.d/resolver.conf
nameserver=$(awk '/^nameserver/ { print $2; exit }' /etc/resolv.conf 2>/dev/null || true)

if [ -z "${nameserver:-}" ]; then
  echo "10-resolver.sh: no nameserver in /etc/resolv.conf; falling back to 127.0.0.11" >&2
  nameserver=127.0.0.11
fi

printf 'resolver %s valid=10s ipv6=off;\n' "$nameserver" > "$resolver_conf"
echo "10-resolver.sh: using resolver $nameserver"

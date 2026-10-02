#!/bin/sh
set -eu
# Secret files are mounted separately into each backend. Never print their values.
for secret in /run/secrets/backend_api_token /run/secrets/brokerrouter_virtual_key; do
    if [ ! -f "$secret" ] || [ ! -r "$secret" ]; then
        echo 'Required backend Secret file is missing or unreadable' >&2
        exit 1
    fi
done
JIACLAW_API_TOKEN=$(cat /run/secrets/backend_api_token)
JIACLAW_API_KEY=$(cat /run/secrets/brokerrouter_virtual_key)
if [ -z "$JIACLAW_API_TOKEN" ] || [ -z "$JIACLAW_API_KEY" ]; then
    echo 'Required backend Secret is empty' >&2
    exit 1
fi
export JIACLAW_API_TOKEN JIACLAW_API_KEY
exec /usr/local/bin/jiaclaw serve --config /etc/jiaclaw/config.toml

#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_root="$(mktemp -d)"
image_tag="sieve-docker-runtime-test:$$-$(date +%s)"

cleanup() {
    docker image rm -f "$image_tag" >/dev/null 2>&1 || true
    rm -rf "$tmp_root"
}

trap cleanup EXIT

require() {
    local pattern="$1"
    local file="$2"
    local message="$3"
    if ! grep -Eq "$pattern" "$file"; then
        echo "$message" >&2
        exit 1
    fi
}

reject() {
    local pattern="$1"
    local file="$2"
    local message="$3"
    if grep -Eq "$pattern" "$file"; then
        echo "$message" >&2
        exit 1
    fi
}

for dockerfile in "$repo_root/Dockerfile" "$repo_root/Dockerfile.release"; do
    reject 'HOME=/root' "$dockerfile" "$dockerfile must not default HOME to /root"
    reject 'SIEVE_HOME=/root/\.sieve' "$dockerfile" "$dockerfile must not default SIEVE_HOME to /root/.sieve"
    reject '/root/\.sieve' "$dockerfile" "$dockerfile must not reference /root/.sieve"
    reject '^USER[[:space:]]+root([[:space:]]|$)' "$dockerfile" "$dockerfile must not run as root"
    require 'HOME=/home/sieve' "$dockerfile" "$dockerfile must default HOME to /home/sieve"
    require 'SIEVE_HOME=/home/sieve/\.sieve' "$dockerfile" "$dockerfile must default SIEVE_HOME to /home/sieve/.sieve"
    require '^USER[[:space:]]+(sieve|1000|1000:1000|\$\{SIEVE_UID\}:\$\{SIEVE_GID\})$' "$dockerfile" "$dockerfile must set a non-root runtime user"
done

build_ctx="$tmp_root/build"
mkdir -p "$build_ctx/docs/policy" "$build_ctx/dist/release/sieve-tools/bin"

cp "$repo_root/Dockerfile.release" "$build_ctx/Dockerfile.release"
cp "$repo_root/.env.example" "$build_ctx/.env.example"
cp -R "$repo_root/docs/policy/." "$build_ctx/docs/policy/"

cat > "$build_ctx/dist/release/sieve-app" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
mkdir -p "$SIEVE_HOME"
touch "$SIEVE_HOME/owned-by-runtime"
EOF
chmod +x "$build_ctx/dist/release/sieve-app"

for tool in codex bravesearch st sieve-lcm-cli; do
    cat > "$build_ctx/dist/release/sieve-tools/bin/$tool" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
    chmod +x "$build_ctx/dist/release/sieve-tools/bin/$tool"
done

docker build -f "$build_ctx/Dockerfile.release" -t "$image_tag" "$build_ctx" >/dev/null

config_user="$(docker image inspect "$image_tag" --format '{{.Config.User}}')"
case "$config_user" in
    ""|root|0|0:0)
        echo "Dockerfile.release must configure a non-root default user" >&2
        exit 1
        ;;
esac

runtime_state="$(docker run --rm --entrypoint /bin/bash "$image_tag" -lc 'printf "%s\n%s\n%s\n" "$HOME" "$SIEVE_HOME" "$(id -u):$(id -g)"')"
runtime_home="$(printf '%s\n' "$runtime_state" | sed -n '1p')"
runtime_sieve_home="$(printf '%s\n' "$runtime_state" | sed -n '2p')"
runtime_ids="$(printf '%s\n' "$runtime_state" | sed -n '3p')"

[[ "$runtime_home" == "/home/sieve" ]] || {
    echo "runtime HOME must be /home/sieve, got $runtime_home" >&2
    exit 1
}
[[ "$runtime_sieve_home" == "/home/sieve/.sieve" ]] || {
    echo "runtime SIEVE_HOME must be /home/sieve/.sieve, got $runtime_sieve_home" >&2
    exit 1
}
[[ "$runtime_ids" == "1000:1000" ]] || {
    echo "runtime user must be 1000:1000, got $runtime_ids" >&2
    exit 1
}

bind_home="$tmp_root/home"
mkdir -p "$bind_home"
if [[ "$(id -u)" == "0" ]]; then
    chown 1000:1000 "$bind_home"
fi
docker run --rm -v "$bind_home:/home/sieve/.sieve" "$image_tag" >/dev/null

file_owner="$(stat -c '%u:%g' "$bind_home/owned-by-runtime")"
[[ "$file_owner" == "1000:1000" ]] || {
    echo "runtime must not create root-owned files in SIEVE_HOME, got $file_owner" >&2
    exit 1
}

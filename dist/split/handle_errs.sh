#!/bin/bash
# handle_errs.sh [cargo args...]: compile (default: -p iyakki-apps --lib) and list the methods / fields that Arc<dyn ServicesHandle> lacks, to /tmp/h_missing.txt
cd "$(dirname "$0")/../.."
args=${@:--p iyakki-apps --lib}
~/iyakki-work/bin/mcargo . check $args --message-format=short 2>&1 | grep -E "^crates.*error|^error" > /tmp/h_err.txt
grep -oE "no method named \`[a-z_0-9]+\` found for (struct|reference) \`(&)?(std::sync::)?Arc<(\(dyn |dyn )ServicesHandle" /tmp/h_err.txt | sed -E 's/no method named `([a-z_0-9]+)`.*/\1/' | sort -u > /tmp/h_missing.txt
grep -oE "no field \`[a-z_0-9]+\` on type \`(&)?(std::sync::)?Arc<(\(dyn |dyn )ServicesHandle" /tmp/h_err.txt | sed -E 's/no field `([a-z_0-9]+)`.*/F:\1/' | sort -u >> /tmp/h_missing.txt
wc -l < /tmp/h_err.txt; echo "missing:"; paste -sd' ' /tmp/h_missing.txt

#!/bin/bash
# Upload the static catalog copy to R2 bothy-data/v1/: files first, the catalog last.
set -euo pipefail
set -a; . ~/.config/bothy-cloudflare.env; set +a
E="https://$CLOUDFLARE_ACCOUNT_ID.r2.cloudflarestorage.com"
S=${1:-$HOME/sp-scratch/publish/v1}   # made by sp-server --export-catalog
A=(aws --profile bothy --region auto --endpoint-url "$E")
echo "start $(date -Is)"
"${A[@]}" s3 sync "$S/download/" s3://bothy-data/v1/download/ --cache-control "public, max-age=3600" --only-show-errors --exclude '*.html' --exclude '*.htm'
# HTML as plain bytes: Cloudflare rewrites text/html (Email Obfuscation), which breaks the hashes
"${A[@]}" s3 sync "$S/download/" s3://bothy-data/v1/download/ --cache-control "public, max-age=3600" --only-show-errors --exclude '*' --include '*.html' --include '*.htm' --content-type application/octet-stream
echo "files done $(date -Is)"
[ -f "$S/api/books" ] && "${A[@]}" s3 cp "$S/api/books" s3://bothy-data/v1/api/books --content-type application/json --cache-control "public, max-age=300" --only-show-errors
"${A[@]}" s3 cp "$S/api/catalog" s3://bothy-data/v1/api/catalog --content-type application/json --cache-control "public, max-age=300" --only-show-errors
echo "catalog done $(date -Is)"
echo DONE

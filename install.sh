#!/bin/bash

URL="https://github.com/michal92299/Universal-Live-Builder/releases/download/v0.1.0/ulb"
TMP_DIR="/tmp/ulb/download-ulb"
BIN_NAME="ulb"

mkdir -p "$TMP_DIR"

# Pobranie
curl -L "$URL" -o "$TMP_DIR/$BIN_NAME"

# Uprawnienia
chmod +x "$TMP_DIR/$BIN_NAME"

# Instalacja
if [ -w /usr/bin ]; then
    echo "System nie wygląda na atomowy → instaluję do /usr/bin"
    sudo mv "$TMP_DIR/$BIN_NAME" /usr/bin/
else
    echo "System jest atomowy → instaluję do ~/.local/bin"
    mkdir -p ~/.local/bin
    mv "$TMP_DIR/$BIN_NAME" ~/.local/bin/
fi


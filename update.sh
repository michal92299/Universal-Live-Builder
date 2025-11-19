#!/usr/bin/env bash

set -euo pipefail

REPO="michal92299/Universal-Live-Builder"
API_URL="https://api.github.com/repos/${REPO}/releases/latest"
INSTALL_SCRIPT_URL="https://raw.githubusercontent.com/michal92299/Universal-Live-Builder/main/install.sh"

# Funkcja: pobierz lokalną wersję ulb
get_local_version() {
    if command -v ulb >/dev/null 2>&1; then
        ulb version
    else
        echo "none"
    fi
}

# Funkcja: pobierz najnowszą wersję z GitHub
get_latest_version() {
    # Pobieramy JSON z GitHub API
    local resp
    resp=$(curl -s -H "Accept: application/vnd.github.v3+json" "$API_URL")
    # Wyciągamy tag_name
    local tag
    tag=$(echo "$resp" | grep '"tag_name":' | head -1 | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')
    echo "$tag"
}

# Funkcja: zainstaluj ulb (uruchom skrypt instalacyjny)
install_ulb() {
    local dest="$1"
    echo "Instalacja ulb do $dest …"
    # Usuwamy istniejącą binarkę
    rm -f "$dest/ulb"
    # Pobieramy instalator
    tmpfile=$(mktemp /tmp/ulb-installer.XXXXXX.sh)
    curl -sL "$INSTALL_SCRIPT_URL" -o "$tmpfile"
    chmod +x "$tmpfile"
    # Uruchamiamy instalator (może wymagać sudo, jeśli instalujesz do /usr/bin)
    if [[ "$dest" == "/usr/bin" ]]; then
        sudo "$tmpfile"
    else
        bash "$tmpfile" --prefix="$HOME/.local"
    fi
    rm -f "$tmpfile"
}

main() {
    local local_ver
    local_ver=$(get_local_version)
    echo "Lokalna wersja ulb: $local_ver"

    local latest_ver
    latest_ver=$(get_latest_version)
    echo "Najnowsza wersja ulb: $latest_ver"

    if [[ "$local_ver" == "none" ]] || [[ "$local_ver" != "$latest_ver" ]]; then
        echo "Nowa wersja dostępna — aktualizacja"
        # Sprawdź, skąd jest ulb
        local ulb_path
        ulb_path=$(command -v ulb || true)

        if [[ -z "$ulb_path" ]]; then
            # Nie znaleziono — zakładamy instalację na domyślnej ścieżce we własnym katalogu
            install_ulb "$HOME/.local/bin"
        elif [[ "$ulb_path" == "$HOME/.local/bin/ulb" ]]; then
            install_ulb "$HOME/.local/bin"
        elif [[ "$ulb_path" == "/usr/bin/ulb" ]]; then
            install_ulb "/usr/bin"
        else
            echo "ulb zainstalowany w niestandardowej lokalizacji: $ulb_path"
            echo "Nie wiem, co z tym zrobić — ręczna aktualizacja będzie lepsza."
            exit 1
        fi
    else
        echo "ulb jest już aktualny."
    fi
}

main "$@"


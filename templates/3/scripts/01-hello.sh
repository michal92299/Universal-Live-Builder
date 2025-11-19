#!/bin/bash
echo "[ULB] Skrypt powitalny podczas budowania..."
echo "[ULB] Twoja własna dystrybucja powstaje właśnie teraz!"

# Zmieniamy prompt na ładniejszy
cat > /etc/profile.d/custom-prompt.sh <<'EOF'
export PS1='\[\e[38;5;208m\]\u\[\e[38;5;15m\]@\[\e[38;5;34m\]\h\[\e[38;5;15m\]:\[\e[38;5;33m\]\w\[\e[0m\]\$ '
EOF

# Ustawiamy domyślny edytor na nano
echo "SETTINGS_EDITOR=nano" >> /etc/environment

echo "[ULB] Konfiguracja zakończona!"

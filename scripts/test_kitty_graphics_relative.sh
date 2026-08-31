#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image="${1:-${repo_root}/crates/desktop_app/examples/assets/kitty-demo.png}"
churn_count="${TERMY_KITTY_CHURN:-1000}"
parent_image_id=42
parent_placement_id=7
child_image_id=43
child_placement_id=8

if [[ ! -f "${image}" ]]; then
  printf 'Image not found: %s\n' "${image}" >&2
  exit 1
fi
if [[ ! "${churn_count}" =~ ^[0-9]+$ ]]; then
  printf 'TERMY_KITTY_CHURN must be a non-negative integer\n' >&2
  exit 1
fi

terminal_cols="$(tput cols 2>/dev/null || printf '80')"
terminal_rows="$(tput lines 2>/dev/null || printf '24')"
image_cols=$((terminal_cols > 44 ? 40 : terminal_cols - 4))
image_rows=$((terminal_rows > 25 ? 20 : terminal_rows - 5))
((image_cols > 0 && image_rows > 0)) || {
  printf 'Terminal is too small for the graphics test\n' >&2
  exit 1
}

cleanup() {
  printf '\033_Ga=d,d=I,i=%d,q=2\033\\' "${child_image_id}"
  printf '\033_Ga=d,d=I,i=%d,q=2\033\\' "${parent_image_id}"
  printf '\033[?25h\033[?1049l'
}
trap cleanup EXIT INT TERM

transmit_png() {
  local first=1
  local chunk next_chunk
  exec 3< <(
    {
      base64 <"${image}" | tr -d '\r\n'
      printf '\n'
    } | fold -w 4096
  )
  IFS= read -r chunk <&3
  while IFS= read -r next_chunk <&3; do
    if ((first)); then
      printf '\033_Ga=t,f=100,t=d,i=%d,q=2,m=1;%s\033\\' "${child_image_id}" "${chunk}"
      first=0
    else
      printf '\033_Gq=2,m=1;%s\033\\' "${chunk}"
    fi
    chunk="${next_chunk}"
  done
  if ((first)); then
    printf '\033_Ga=t,f=100,t=d,i=%d,q=2,m=0;%s\033\\' "${child_image_id}" "${chunk}"
  else
    printf '\033_Gq=2,m=0;%s\033\\' "${chunk}"
  fi
  exec 3<&-
}

place_relative_image() {
  printf '\033_Ga=p,i=%d,p=%d,P=%d,Q=%d,H=0,V=0,c=%d,r=%d,C=1,q=2\033\\' \
    "${child_image_id}" "${child_placement_id}" \
    "${parent_image_id}" "${parent_placement_id}" \
    "${image_cols}" "${image_rows}"
}

printf '\033[?1049h\033[?25l\033[2J\033[H'

# Register a transparent virtual placement, then put its single Unicode
# placeholder cell at row 2, column 3. The real image is anchored to it through
# P/Q, exactly the mode used by TUIs that redraw their image regions as text.
printf '\033_Ga=T,f=32,s=1,v=1,i=%d,p=%d,U=1,c=1,r=1,C=1,q=2;AAAAAA==\033\\' \
  "${parent_image_id}" "${parent_placement_id}"
printf '\033[2;3H\033[38;5;%d;58;5;%dm\xf4\x8e\xbb\xae\xcc\x85\xcc\x85\033[39;59m' \
  "${parent_image_id}" "${parent_placement_id}"

transmit_png
place_relative_image

printf '\033[%d;1HRelative image should start at row 2, column 3. Press Enter to run %d redraws.' \
  "${terminal_rows}" "${churn_count}"
IFS= read -r _

for ((iteration = 0; iteration < churn_count; iteration++)); do
  place_relative_image
done

printf '\033[%d;1HRedraw complete. Press Enter to erase the placeholder; the image must disappear.\033[K' \
  "${terminal_rows}"
IFS= read -r _
printf '\033[2;3H '

printf '\033[%d;1HImage should now be gone. Press Enter to exit.\033[K' "${terminal_rows}"
IFS= read -r _

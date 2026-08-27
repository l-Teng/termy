#!/usr/bin/env bash

set -euo pipefail

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
  printf '%s\n' \
    'Usage: scripts/test_kitty_clipboard.sh [--file-url-repro]' \
    '' \
    'Run this inside Termy. The script tests OSC 5522 mode negotiation and' \
    'clipboard format listing, then offers interactive read, paste-event,' \
    'and multi-format write tests.' \
    '' \
    '--file-url-repro skips unrelated checks and validates the file URL from' \
    'one MIME-aware paste event.'
  exit 0
fi

file_url_repro=0
if [[ "${1:-}" == "--file-url-repro" ]]; then
  file_url_repro=1
  shift
fi

if [[ $# -ne 0 ]]; then
  printf 'Unknown argument: %s\n' "$1" >&2
  exit 2
fi

if [[ ! -t 0 || ! -e /dev/tty ]]; then
  printf 'Run this script interactively inside Termy.\n' >&2
  exit 1
fi

if ! command -v base64 >/dev/null 2>&1; then
  printf 'This test needs the base64 command.\n' >&2
  exit 1
fi

if ! command -v python3 >/dev/null 2>&1; then
  printf 'This test needs python3 to validate file URLs.\n' >&2
  exit 1
fi

if printf '' | base64 --decode >/dev/null 2>&1; then
  base64_decode() { base64 --decode; }
elif printf '' | base64 -D >/dev/null 2>&1; then
  base64_decode() { base64 -D; }
else
  printf 'Could not find a supported base64 decode flag.\n' >&2
  exit 1
fi

base64_encode() {
  base64 | tr -d '\r\n'
}

exec 3<>/dev/tty

tty_state="$(stty -g <&3)"
raw_mode=0
mode_touched=0
initial_mode=2
overall_status=0

enter_raw_mode() {
  stty raw -echo <&3
  raw_mode=1
}

leave_raw_mode() {
  if ((raw_mode)); then
    stty "${tty_state}" <&3
    raw_mode=0
  fi
}

restore_terminal() {
  trap - EXIT INT TERM
  leave_raw_mode || true
  if ((mode_touched)); then
    if [[ "${initial_mode}" == "1" ]]; then
      printf '\033[?5522h' >&3
    else
      printf '\033[?5522l' >&3
    fi
  fi
  exec 3>&-
}

trap restore_terminal EXIT
trap 'exit 130' INT TERM

prompt_yes() {
  local answer
  printf '%s [y/N] ' "$1" >&3
  IFS= read -r answer <&3 || return 1
  [[ "${answer}" == "y" || "${answer}" == "Y" ]]
}

send_osc() {
  printf '\033]5522;%s\033\\' "$1" >&3
}

query_mode() {
  local response
  printf '\033[?5522$p' >&3
  if ! IFS= read -r -d 'y' -t 5 response <&3; then
    return 1
  fi
  response="${response}y"
  case "${response}" in
    $'\033[?5522;1$y') queried_mode=1 ;;
    $'\033[?5522;2$y') queried_mode=2 ;;
    *)
      queried_mode=0
      return 1
      ;;
  esac
}

read_osc_response() {
  local prefix terminator
  if ! IFS= read -r -n 2 -t 300 prefix <&3; then
    return 1
  fi
  if [[ "${prefix}" != $'\033]' ]]; then
    return 1
  fi
  if ! IFS= read -r -d $'\033' -t 300 osc_body <&3; then
    return 1
  fi
  if ! IFS= read -r -n 1 -t 5 terminator <&3; then
    return 1
  fi
  [[ "${terminator}" == '\' && "${osc_body}" == 5522\;* ]]
}

field_value() {
  local metadata="$1"
  local wanted="$2"
  local record
  local records=()
  IFS=':' read -r -a records <<<"${metadata}"
  for record in "${records[@]}"; do
    if [[ "${record}" == "${wanted}="* ]]; then
      printf '%s' "${record#*=}"
      return 0
    fi
  done
  return 1
}

parse_osc_response() {
  local response="${osc_body#5522;}"
  if [[ "${response}" == *';'* ]]; then
    osc_metadata="${response%%;*}"
    osc_payload="${response#*;}"
  else
    osc_metadata="${response}"
    osc_payload=""
  fi
}

flush_read_mime() {
  if [[ -n "${read_current_mime}" ]]; then
    read_summary="${read_summary}${read_current_mime}: ${read_current_bytes} bytes"$'\n'
    read_current_mime=""
    read_current_bytes=0
  fi
}

consume_read_responses() {
  local status encoded_mime mime chunk_bytes password decoded_chunk
  local response_count=0

  read_error=""
  read_formats=""
  read_password=""
  read_summary=""
  read_total_bytes=0
  read_current_mime=""
  read_current_bytes=0
  read_uri_list=""

  while ((response_count < 65536)); do
    response_count=$((response_count + 1))
    if ! read_osc_response; then
      read_error="timed out or received a malformed OSC response"
      return 1
    fi
    parse_osc_response
    status="$(field_value "${osc_metadata}" status || true)"
    password="$(field_value "${osc_metadata}" pw || true)"
    if [[ -n "${password}" ]]; then
      read_password="${password}"
    fi

    case "${status}" in
      OK) ;;
      DATA)
        encoded_mime="$(field_value "${osc_metadata}" mime || true)"
        if [[ -z "${encoded_mime}" ]] || ! mime="$(printf '%s' "${encoded_mime}" | base64_decode)"; then
          read_error="DATA response had an invalid MIME type"
          return 1
        fi
        if [[ "${mime}" == "." ]]; then
          if ! read_formats="$(printf '%s' "${osc_payload}" | base64_decode)"; then
            read_error="format list was not valid Base64"
            return 1
          fi
          continue
        fi
        if ! chunk_bytes="$(printf '%s' "${osc_payload}" | base64_decode | wc -c | tr -d '[:space:]')"; then
          read_error="clipboard payload was not valid Base64"
          return 1
        fi
        if [[ "${read_current_mime}" != "${mime}" ]]; then
          flush_read_mime
          read_current_mime="${mime}"
        fi
        if [[ "${mime}" == "text/uri-list" ]]; then
          if ! decoded_chunk="$(printf '%s' "${osc_payload}" | base64_decode)"; then
            read_error="clipboard URI list was not valid Base64"
            return 1
          fi
          read_uri_list="${read_uri_list}${decoded_chunk}"
        fi
        read_current_bytes=$((read_current_bytes + chunk_bytes))
        read_total_bytes=$((read_total_bytes + chunk_bytes))
        ;;
      DONE)
        flush_read_mime
        return 0
        ;;
      EPERM|ENOSYS|EBUSY)
        read_error="terminal returned ${status}"
        return 1
        ;;
      *)
        read_error="terminal returned an unexpected status: ${status:-missing}"
        return 1
        ;;
    esac
  done

  read_error="response limit exceeded"
  return 1
}

first_file_url_path() {
  TERMY_TEST_URI_LIST="${read_uri_list}" python3 - <<'PY'
import os
import sys
from urllib.parse import unquote, urlsplit

for raw_line in os.environ.get("TERMY_TEST_URI_LIST", "").splitlines():
    line = raw_line.strip()
    if not line or line.startswith("#"):
        continue
    parsed = urlsplit(line)
    if parsed.scheme != "file" or parsed.netloc not in ("", "localhost"):
        sys.exit(2)
    sys.stdout.write(unquote(parsed.path))
    sys.exit(0)
sys.exit(3)
PY
}

verify_uri_list_target() {
  local path prefix
  if [[ -z "${read_uri_list}" ]]; then
    printf 'INCONCLUSIVE: no text/uri-list payload was returned.\n'
    return 2
  fi
  if ! path="$(first_file_url_path)"; then
    printf 'FAIL: text/uri-list did not contain a valid local file URL.\n' >&2
    return 1
  fi
  if ! prefix="$(dd if="${path}" bs=1 count=8 status=none 2>/dev/null | od -An -tx1 | tr -d '[:space:]')"; then
    if [[ "${path}" == *'/TemporaryItems/NSIRD_'* ]]; then
      printf 'REPRODUCED: Termy exposed an unreadable macOS screenshot temporary URL.\n' >&2
    else
      printf 'REPRODUCED: Termy exposed a clipboard file URL the child process cannot read.\n' >&2
    fi
    return 1
  fi
  if [[ -z "${prefix}" ]]; then
    printf 'REPRODUCED: Termy exposed an empty clipboard file URL target.\n' >&2
    return 1
  fi
  if [[ "${path}" == *'/TemporaryItems/NSIRD_'* ]]; then
    printf 'REPRODUCED: Termy forwarded a provider-owned macOS screenshot temporary URL.\n' >&2
    return 1
  fi
  if [[ "${prefix}" != 89504e470d0a1a0a* ]]; then
    printf 'INCONCLUSIVE: the clipboard file URL is readable but is not a PNG.\n'
    return 2
  fi
  printf 'PASS: clipboard image URL points to a readable PNG outside macOS TemporaryItems.\n'
}

print_read_summary() {
  if [[ -n "${read_summary}" ]]; then
    printf '%s' "${read_summary}"
  else
    printf 'No matching clipboard data was returned.\n'
  fi
}

printf '%s\n' \
  'Kitty clipboard protocol test' \
  'Run this in Termy and keep this terminal focused while answering prompts.' \
  ''

if ((file_url_repro)); then
  printf '\033[?5522h' >&3
  mode_touched=1
  printf 'Paste-event mode enabled for the focused file URL reproduction.\n'
else
  enter_raw_mode
  if ! query_mode; then
    leave_raw_mode
    printf 'FAIL: Termy did not answer the private mode 5522 query.\n' >&2
    exit 1
  fi
  initial_mode="${queried_mode}"
  printf '\033[?5522h' >&3
  mode_touched=1
  if ! query_mode || [[ "${queried_mode}" != "1" ]]; then
    leave_raw_mode
    printf 'FAIL: Termy did not enable private mode 5522.\n' >&2
    exit 1
  fi
  leave_raw_mode
  printf 'PASS: private mode 5522 negotiation\n'
fi

if ((!file_url_repro)); then
  enter_raw_mode
  send_osc 'type=read:id=formats;Lg=='
  if consume_read_responses; then
    formats_ok=1
  else
    formats_ok=0
  fi
  leave_raw_mode
  if ((formats_ok)); then
    printf 'PASS: clipboard format listing\n'
    printf 'Formats: %s\n' "${read_formats:-none reported}"
  else
    printf 'FAIL: clipboard format listing, %s\n' "${read_error}" >&2
  fi
fi

test_name="$(printf 'Termy clipboard test' | base64_encode)"
test_password="$(printf 'termy-clipboard-test-%s' "$$" | base64_encode)"

printf '\n'
if ((!file_url_repro)) && prompt_yes 'Trigger a permission prompt for reading text/plain?'; then
  enter_raw_mode
  send_osc "type=read:id=permission-read:pw=${test_password}:name=${test_name};dGV4dC9wbGFpbg=="
  if consume_read_responses; then
    permission_read_ok=1
  else
    permission_read_ok=0
  fi
  leave_raw_mode
  if ((permission_read_ok)); then
    printf 'PASS: permissioned clipboard read\n'
    print_read_summary
  else
    printf 'Read did not complete: %s\n' "${read_error}"
  fi
fi

printf '\n'
if ((file_url_repro)) || prompt_yes 'Test a MIME-aware paste event and its single-use read token?'; then
  printf 'Press your normal Paste shortcut now. Do not press Enter.\n'
  enter_raw_mode
  if consume_read_responses; then
    paste_event_ok=1
  else
    paste_event_ok=0
  fi

  if ((paste_event_ok)) && [[ -n "${read_password}" && -n "${read_formats}" ]]; then
    paste_password="${read_password}"
    paste_formats="${read_formats}"
    if ((file_url_repro)); then
      paste_request='dGV4dC91cmktbGlzdA=='
    else
      paste_request="$(printf '%s' "${paste_formats}" | base64_encode)"
    fi
    send_osc "type=read:id=paste-read:pw=${paste_password}:name=${test_name};${paste_request}"
    if consume_read_responses; then
      paste_read_ok=1
    else
      paste_read_ok=0
    fi
  else
    paste_read_ok=0
  fi
  leave_raw_mode

  if ((paste_event_ok)); then
    printf 'PASS: MIME-aware paste event\n'
    printf 'Advertised formats: %s\n' "${paste_formats:-none reported}"
  else
    printf 'FAIL: MIME-aware paste event, %s\n' "${read_error}" >&2
  fi
  if ((paste_read_ok)); then
    printf 'PASS: paste token read without another permission prompt\n'
    print_read_summary
    if [[ "${paste_formats}" == *'text/uri-list'* ]]; then
      if verify_uri_list_target; then
        paste_uri_status=0
      else
        paste_uri_status=$?
        overall_status="${paste_uri_status}"
      fi
    else
      printf 'INCONCLUSIVE: paste event did not advertise text/uri-list.\n'
      paste_uri_status=2
      overall_status=2
    fi
  elif ((paste_event_ok)); then
    printf 'Token read did not complete: %s\n' "${read_error:-no readable formats}"
  fi
fi

printf '\n'
if ((!file_url_repro)) && prompt_yes 'Overwrite the clipboard with test text and a 1x1 PNG?'; then
  text_payload="$(printf 'Termy Kitty clipboard protocol test %s' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" | base64_encode)"
  png_payload='iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII='
  alias_payload="$(printf 'text/utf8' | base64_encode)"

  enter_raw_mode
  send_osc "type=write:id=write-test:pw=${test_password}:name=${test_name}"
  send_osc "type=walias:mime=dGV4dC9wbGFpbg==;${alias_payload}"
  send_osc "type=wdata:mime=dGV4dC9wbGFpbg==;${text_payload}"
  send_osc "type=wdata:mime=aW1hZ2UvcG5n;${png_payload}"
  send_osc 'type=wdata'
  if read_osc_response; then
    parse_osc_response
    write_status="$(field_value "${osc_metadata}" status || true)"
  else
    write_status="missing response"
  fi
  leave_raw_mode

  if [[ "${write_status}" == "DONE" ]]; then
    printf 'PASS: multi-format write completed\n'
    printf 'The clipboard now offers text/plain, its text/utf8 alias, and image/png.\n'
  else
    printf 'FAIL: multi-format write returned %s\n' "${write_status}" >&2
  fi
fi

printf '\nKitty clipboard test finished.\n'
exit "${overall_status}"

#!/usr/bin/env bash
#
# smoke-test.sh — Automated boot + command test for MerlionOS
# Boots QEMU, sends commands via monitor, checks serial output.
#
set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m'

KERNEL_BIN="target/x86_64-unknown-none/debug/bootimage-merlion-kernel.bin"
SERIAL_LOG="/tmp/merlion-smoke-test.log"
MON_SOCK="/tmp/merlion-smoke-mon.sock"
TIMEOUT=30
PASSED=0
FAILED=0

ok()   { PASSED=$((PASSED + 1)); printf "${GREEN}[PASS]${NC} %s\n" "$1"; }
fail() { FAILED=$((FAILED + 1)); printf "${RED}[FAIL]${NC} %s\n" "$1"; }

cleanup() {
    kill $QEMU_PID 2>/dev/null || true
    rm -f "$MON_SOCK" "$SERIAL_LOG"
}
trap cleanup EXIT

# Build
echo "Building kernel..."
cargo bootimage --bin merlion-kernel 2>&1 | tail -1

if [ ! -f "$KERNEL_BIN" ]; then
    echo "ERROR: Kernel binary not found at $KERNEL_BIN"
    exit 1
fi

# Start QEMU
echo "Starting QEMU..."
rm -f "$MON_SOCK" "$SERIAL_LOG"
qemu-system-x86_64 \
    -drive format=raw,file="$KERNEL_BIN" \
    -serial file:"$SERIAL_LOG" \
    -display none \
    -m 256M \
    -monitor unix:"$MON_SOCK",server,nowait &
QEMU_PID=$!
sleep 1

# Helper: send key via monitor
sendkey() {
    echo "sendkey $1" | socat - UNIX-CONNECT:"$MON_SOCK" 2>/dev/null
    sleep 0.15
}

# Helper: send string as keystrokes
sendstr() {
    for (( i=0; i<${#1}; i++ )); do
        local ch="${1:$i:1}"
        case "$ch" in
            ' ') sendkey spc ;;
            '.') sendkey dot ;;
            '/') sendkey slash ;;
            '-') sendkey minus ;;
            '_') sendkey shift-minus ;;
            '=') sendkey equal ;;
            *) sendkey "$ch" ;;
        esac
    done
}

# Helper: check if serial log contains pattern
check() {
    if grep -q "$1" "$SERIAL_LOG" 2>/dev/null; then
        ok "$2"
    else
        fail "$2 (pattern: $1)"
    fi
}

# Wait for boot
echo "Waiting for boot..."
sleep 8

# Test 1: Kernel boots
check "Kernel initialization complete" "Kernel boots successfully"
check "Heap allocator initialized" "Heap allocator works"

# Login: root + empty password
echo "Logging in..."
sendstr "root"
sendkey ret
sleep 1
sendkey ret  # empty password
sleep 2

check "authenticated successfully" "Login works"

# Test 2: Basic commands
echo "Testing commands..."

sendstr "help"
sendkey ret
sleep 1
check "shell:" "Shell dispatch works"

sendstr "info"
sendkey ret
sleep 1

sendstr "uptime"
sendkey ret
sleep 1

sendstr "ps"
sendkey ret
sleep 1

sendstr "heap"
sendkey ret
sleep 1

sendstr "ls /"
sendkey ret
sleep 1

sendstr "cat /proc/version"
sendkey ret
sleep 1

sendstr "whoami"
sendkey ret
sleep 1

# Wait for output
sleep 2

# Check results
check "shell: help" "help command dispatched"
check "shell: info" "info command dispatched"
check "shell: uptime" "uptime command dispatched"
check "shell: ps" "ps command dispatched"
check "shell: heap" "heap command dispatched"
check "shell: ls /" "ls command dispatched"
check "shell: whoami" "whoami command dispatched"

# Summary
echo ""
echo "================================"
echo "Smoke Test Results"
echo "================================"
echo "Passed: $PASSED"
echo "Failed: $FAILED"
echo "Total:  $((PASSED + FAILED))"
echo "================================"

if [ $FAILED -gt 0 ]; then
    echo ""
    echo "Serial log:"
    tail -30 "$SERIAL_LOG"
    exit 1
fi

echo ""
echo "All tests passed!"

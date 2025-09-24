#!/bin/bash

# Terminal Size Test Script
# Run this inside the container to test terminal sizing

echo "=== Terminal Size Test ==="
echo

# Check environment variables
echo "Environment Variables:"
echo "  COLUMNS: ${COLUMNS:-'not set'}"
echo "  LINES: ${LINES:-'not set'}"
echo "  TERM: ${TERM:-'not set'}"
echo

# Check tput output
echo "tput Commands:"
echo "  tput cols: $(tput cols 2>/dev/null || echo 'failed')"
echo "  tput lines: $(tput lines 2>/dev/null || echo 'failed')"
echo

# Check stty output
echo "stty size:"
stty size 2>/dev/null || echo "stty failed"
echo

# Visual test with ruler
echo "Visual Width Test (should show up to column number):"
for i in {1..9}; do printf "%d%09d" $i $(($i * 10)); done
echo
echo "         ^         ^         ^         ^         ^         ^         ^         ^         ^         ^         ^         ^"
echo "        10        20        30        40        50        60        70        80        90       100       110       120"
echo

# Test line wrapping
echo "Text Wrapping Test:"
echo "This is a very long line that should wrap properly at the terminal width without causing overlapping text issues or incorrect formatting problems that make the terminal hard to read."
echo

echo "If you see text wrapping at the correct width and no overlapping, terminal sizing is working!"
echo "If terminal size is wrong, run: resize_terminal [cols] [rows]"
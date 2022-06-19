#!/bin/bash

set -eo pipefail

if [ -z $1 ]; then
    OUT=man/i3status-rs.1
else
    OUT=$1
fi

(cd gen-manpage && cargo run -- ../src ../man)

# TODO: fix deprecation warning
pandoc -o man/themes.1 -t man --base-header-level=2 doc/themes.md

# Delete the table of contents from the block documentation.
sed -i '0,/xrandr/d' man/blocks.1

# Add appropriate section headers.
sed -i '1i .SH BLOCKS\n' man/blocks.1
sed -i '1i .SH THEMES\n' man/themes.1

# Stich together the final manpage.
cat man/_preface.1 man/blocks.1 man/themes.1 man/_postface.1 > $OUT

rm man/blocks.1 man/themes.1

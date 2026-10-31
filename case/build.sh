#!/bin/sh
# Exports every printable part of the case to stl/, in print orientation.
set -e
cd "$(dirname "$0")"
mkdir -p stl
for part in shell back bar caps pins fit_window fit_ports; do
    echo "$part"
    openscad -q -o "stl/$part.stl" -D "part=\"$part\"" oxidized-ink-case.scad
done

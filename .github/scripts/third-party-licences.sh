#!/usr/bin/env bash
# Writes THIRD_PARTY_LICENSES.txt at the top of the repository: the licences of everything the server's program contains, which most
# of them ask to be kept with every copy of it. The crates come from cargo-about (about.toml, about.hbs), which fails if one has a
# licence that is not on its list, so a new dependency's licence is looked at before it is shipped. The fonts are not crates, so they
# are added here. Run from the repository root, with cargo-about installed.
set -euo pipefail

out=THIRD_PARTY_LICENSES.txt
cargo about generate --locked about.hbs --output-file "$out"

{
    printf '\n\n'
    printf '%s\n' '================================================================================'
    printf '%s\n' 'Fonts: Overpass Light and SemiBold (inside the program, for the built-in layouts)'
    printf '%s\n' '================================================================================'
    printf '\n'
    cat templates/fonts/Overpass/OFL.txt
    printf '\n\n'
    printf '%s\n' '================================================================================'
    printf '%s\n' 'Weather icons: Meteocons by Bas Milius (inside the program)'
    printf '%s\n' '================================================================================'
    printf '\n'
    cat templates/weather_icons/LICENSE
} >> "$out"

wc -c "$out"

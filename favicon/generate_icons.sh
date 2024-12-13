#!/bin/bash
set -euo pipefail
# Very short script generating the different favicon sizes indicated here:
# https://stackoverflow.com/a/68189611
# Uses the inkscape CLI to perform conversion, make sure you have that
# installed if you want to regenerate the icons from the 'favicon.svg'.
inkscape -w 16  -h 16  favicon.svg -o favicon-16.png
inkscape -w 32  -h 32  favicon.svg -o favicon-32.png
inkscape -w 48  -h 48  favicon.svg -o favicon-48.png
inkscape -w 167 -h 167 favicon.svg -o favicon-167.png
inkscape -w 180 -h 180 favicon.svg -o favicon-180.png
inkscape -w 192 -h 192 favicon.svg -o favicon-192.png
# Postprocess the generated PNGs by reducing their file size with oxipng.
oxipng *.png

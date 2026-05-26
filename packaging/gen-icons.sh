#!/bin/sh
# gen-icons.sh - generate platform icons from a single SVG source.
#
# Produces:
#   packaging/icons/c0pl4nd-<size>.png  (16..1024)
#   packaging/icons/c0pl4nd.png         (256x256 canonical)
#   packaging/windows/c0pl4nd.ico       (multi-size Windows icon)
#   packaging/macos/c0pl4nd.icns        (macOS icon set)
#
# POSIX sh. Source: assets/svg/app-icon.svg
#
# Required tools on the maintainer's machine (auto-detected):
#   * rsvg-convert  (librsvg)  OR  magick/convert  (ImageMagick)  -- SVG -> PNG
#   * ImageMagick (magick/convert)                               -- PNG -> ICO
#   * iconutil (macOS, ships with Xcode)  OR  png2icns (libicns) -- PNG -> ICNS
#
# If a tool is missing, the corresponding step is skipped with a warning.
set -eu

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
CRATE_DIR="$(CDPATH= cd -- "${SCRIPT_DIR}/.." && pwd)"

SVG="${SVG:-${CRATE_DIR}/assets/svg/app-icon.svg}"
ICON_DIR="${SCRIPT_DIR}/icons"
WIN_ICO="${SCRIPT_DIR}/windows/c0pl4nd.ico"
MAC_ICNS="${SCRIPT_DIR}/macos/c0pl4nd.icns"

SIZES="16 24 32 48 64 128 256 512 1024"

if [ ! -f "${SVG}" ]; then
	echo "error: SVG source not found at ${SVG}" >&2
	echo "       create assets/svg/app-icon.svg or set SVG=<path>" >&2
	exit 1
fi

mkdir -p "${ICON_DIR}"

# --- pick an SVG rasterizer ----------------------------------------------
RASTER=""
if command -v rsvg-convert >/dev/null 2>&1; then
	RASTER="rsvg"
elif command -v magick >/dev/null 2>&1; then
	RASTER="magick"
elif command -v convert >/dev/null 2>&1; then
	RASTER="convert"
else
	echo "error: no SVG rasterizer found (install librsvg or ImageMagick)" >&2
	exit 1
fi

render_png() {
	# render_png <size> <output>
	size="$1"
	out="$2"
	case "${RASTER}" in
		rsvg)
			rsvg-convert -w "${size}" -h "${size}" "${SVG}" -o "${out}"
			;;
		magick)
			magick -background none -density 384 "${SVG}" \
				-resize "${size}x${size}" "${out}"
			;;
		convert)
			convert -background none -density 384 "${SVG}" \
				-resize "${size}x${size}" "${out}"
			;;
	esac
}

echo "Rendering PNG sizes with: ${RASTER}"
for s in ${SIZES}; do
	out="${ICON_DIR}/c0pl4nd-${s}.png"
	render_png "${s}" "${out}"
	echo "  ${out}"
done
cp "${ICON_DIR}/c0pl4nd-256.png" "${ICON_DIR}/c0pl4nd.png"
echo "  ${ICON_DIR}/c0pl4nd.png (canonical 256)"

# --- Windows .ico ---------------------------------------------------------
ICO_TOOL=""
if command -v magick >/dev/null 2>&1; then
	ICO_TOOL="magick"
elif command -v convert >/dev/null 2>&1; then
	ICO_TOOL="convert"
fi

if [ -n "${ICO_TOOL}" ]; then
	mkdir -p "$(dirname "${WIN_ICO}")"
	"${ICO_TOOL}" \
		"${ICON_DIR}/c0pl4nd-16.png" \
		"${ICON_DIR}/c0pl4nd-32.png" \
		"${ICON_DIR}/c0pl4nd-48.png" \
		"${ICON_DIR}/c0pl4nd-64.png" \
		"${ICON_DIR}/c0pl4nd-128.png" \
		"${ICON_DIR}/c0pl4nd-256.png" \
		"${WIN_ICO}"
	echo "Wrote ${WIN_ICO}"
else
	echo "warning: ImageMagick not found; skipping .ico generation" >&2
fi

# --- macOS .icns ----------------------------------------------------------
if command -v iconutil >/dev/null 2>&1; then
	ICONSET="${ICON_DIR}/c0pl4nd.iconset"
	rm -rf "${ICONSET}"
	mkdir -p "${ICONSET}"
	# Apple iconset naming convention (base + @2x retina variants).
	cp "${ICON_DIR}/c0pl4nd-16.png"   "${ICONSET}/icon_16x16.png"
	cp "${ICON_DIR}/c0pl4nd-32.png"   "${ICONSET}/icon_16x16@2x.png"
	cp "${ICON_DIR}/c0pl4nd-32.png"   "${ICONSET}/icon_32x32.png"
	cp "${ICON_DIR}/c0pl4nd-64.png"   "${ICONSET}/icon_32x32@2x.png"
	cp "${ICON_DIR}/c0pl4nd-128.png"  "${ICONSET}/icon_128x128.png"
	cp "${ICON_DIR}/c0pl4nd-256.png"  "${ICONSET}/icon_128x128@2x.png"
	cp "${ICON_DIR}/c0pl4nd-256.png"  "${ICONSET}/icon_256x256.png"
	cp "${ICON_DIR}/c0pl4nd-512.png"  "${ICONSET}/icon_256x256@2x.png"
	cp "${ICON_DIR}/c0pl4nd-512.png"  "${ICONSET}/icon_512x512.png"
	cp "${ICON_DIR}/c0pl4nd-1024.png" "${ICONSET}/icon_512x512@2x.png"
	mkdir -p "$(dirname "${MAC_ICNS}")"
	iconutil -c icns "${ICONSET}" -o "${MAC_ICNS}"
	echo "Wrote ${MAC_ICNS}"
elif command -v png2icns >/dev/null 2>&1; then
	mkdir -p "$(dirname "${MAC_ICNS}")"
	png2icns "${MAC_ICNS}" \
		"${ICON_DIR}/c0pl4nd-16.png" \
		"${ICON_DIR}/c0pl4nd-32.png" \
		"${ICON_DIR}/c0pl4nd-48.png" \
		"${ICON_DIR}/c0pl4nd-128.png" \
		"${ICON_DIR}/c0pl4nd-256.png" \
		"${ICON_DIR}/c0pl4nd-512.png" \
		"${ICON_DIR}/c0pl4nd-1024.png"
	echo "Wrote ${MAC_ICNS}"
else
	echo "warning: neither iconutil nor png2icns found; skipping .icns" >&2
fi

echo "Done."

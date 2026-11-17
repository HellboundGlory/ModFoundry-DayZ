#!/usr/bin/env bash
set -euo pipefail

: "${MODFOUNDRY_DATA_DIR:-${XDG_DATA_HOME:-${HOME:?set HOME or MODFOUNDRY_DATA_DIR}}}"
root="$(realpath -m "${MODFOUNDRY_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/modfoundry}/cuda")"
PATH="$PATH:/usr/sbin:/sbin"

cache="$(ldconfig -p 2>/dev/null || true)"
if ! grep -q '^[[:space:]]*libcuda\.so\.1[[:space:]]' <<<"$cache"; then
  echo "NVIDIA driver not found: libcuda.so.1 is not in the loader cache." >&2
  echo "On Nobara, install the driver with the Nobara Driver Manager, then rerun this script." >&2
  exit 1
fi

if ! python3 -m pip --version >/dev/null 2>&1; then
  echo "pip is needed to fetch the CUDA libraries: sudo dnf install python3-pip" >&2
  exit 1
fi

echo "About 800 MB will be downloaded into $root/wheels."
python3 -m pip install --upgrade --no-deps --target "$root/wheels" \
  nvidia-cuda-runtime==13.2.86 nvidia-cuda-nvrtc==13.2.86 nvidia-cublas==13.6.0.2 nvidia-curand==10.4.3.29

mkdir -p "$root/lib"
find "$root/lib" -xtype l -delete
# find runs on its own so set -e sees it fail; inside a process substitution it would not.
links="$root/links.list"
trap 'rm -f "$links"' EXIT
find "$root/wheels" -name 'lib*.so*' -print0 >"$links"
while IFS= read -r -d '' file; do
  ln -sfn "$file" "$root/lib/"
done <"$links"

missing=()
for lib in libcudart.so.13 libcublas.so.13 libcublasLt.so.13 libcurand.so.10 libnvrtc.so.13; do
  [ -e "$root/lib/$lib" ] || missing+=("$lib")
done
if [ "${#missing[@]}" -gt 0 ]; then
  echo "missing: ${missing[*]}" >&2
  exit 1
fi
echo "CUDA libraries ready in $root/lib"

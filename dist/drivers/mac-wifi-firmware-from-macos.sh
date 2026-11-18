#!/bin/bash
# Copy the Broadcom Wi-Fi (and, where Linux needs it, Bluetooth) firmware of a T2 Mac out of macOS, under the file names
# Linux asks for, onto a USB stick or into a folder.
#
#   Run in macOS (Terminal), on the same Mac:   bash mac-wifi-firmware-from-macos.sh /Volumes/<STICK>
#
# Then in IYAKKI: STORE -> Drivers -> Wi-Fi -> "Install from folder or USB" and pick <STICK>/iyakki-wifi-firmware
# (and the Bluetooth entry for the brcmbt* files, when there are any).
# The files are Apple's; they stay on your own machine and are not part of IYAKKI.
#
# Chips: BCM4364 (B2: Mac mini 2018, MacBookPro15,1/15,2/15,3, iMac Pro...; B3: MacBookPro16,1/16,2...), BCM4377 (B3:
# MacBookPro15,4, MacBookPro16,3, MacBookAir9,1), BCM4355 (C1). Naming follows the t2linux / Asahi Linux firmware script
# (https://github.com/t2linux/wiki, docs/tools/firmware.sh) and what brcmfmac and hci_bcm4377 request in Linux 6.12.
# SPDX-License-Identifier: Apache-2.0
set -eu

dest="${1:-$HOME/Desktop}/iyakki-wifi-firmware"
src="${IYAKKI_FW_SRC:-/usr/share/firmware}"        # IYAKKI_FW_SRC: only for testing on a copy of that folder
[ -d "$src/wifi" ] || { echo "no $src/wifi - run this in macOS on the Mac itself" >&2; exit 1; }
mkdir -p "$dest/raw"

lower() { printf '%s' "$1" | tr 'A-Z' 'a-z'; }

# ---- Wi-Fi (brcmfmac) --------------------------------------------------------------------------------------------------
# Apple's NVRAM file name carries what brcmfmac puts in its request:
#   C-<chip>__s-<rev>/P-<platform>-<antenna>_M-<module>_V-<vendor>__m-<version>.txt
#   -> brcmfmac<chip><rev>-pcie.apple,<platform>-<module>-<vendor>-<version>-<antenna>.{bin,txt,clm_blob,txcap_blob}
# (the first, most specific name brcmfmac tries; the firmware is <platform>.trx, the blobs <platform>[-<antenna>].clmb/.txcb).
found=0
for chipdir in "$src"/wifi/C-4364__s-* "$src"/wifi/C-4377__s-* "$src"/wifi/C-4355__s-*; do
    [ -d "$chipdir" ] || continue
    d=$(basename "$chipdir")                                     # C-4364__s-B2
    chip=$(printf '%s' "$d" | sed -E 's/^C-([0-9]+)__s-.*/\1/')
    rev=$(lower "$(printf '%s' "$d" | sed 's/.*__s-//')")        # B2 -> b2
    cp -R "$chipdir" "$dest/raw/"
    for nv in "$chipdir"/P-*_M-*_V-*__m-*.txt; do
        [ -f "$nv" ] || continue
        n=$(basename "$nv" .txt)
        platform=$(printf '%s' "$n" | sed -E 's/^P-([^-_]+)-.*/\1/')
        antenna=$(printf '%s' "$n" | sed -E 's/^P-[^-_]+-([^_]+)_M-.*/\1/')
        module=$(printf '%s' "$n" | sed -E 's/.*_M-([^_]+)_V-.*/\1/')
        vendor=$(printf '%s' "$n" | sed -E 's/.*_V-([^_]+)__m-.*/\1/')
        version=$(printf '%s' "$n" | sed -E 's/.*__m-(.*)$/\1/')
        base="brcmfmac${chip}${rev}-pcie.apple,${platform}-${module}-${vendor}-${version}-${antenna}"
        trx="$chipdir/$platform.trx"
        [ -f "$trx" ] || { echo "skip $n: no $platform.trx" >&2; continue; }
        cp "$trx" "$dest/$base.bin"
        # brcmfmac does not like spaces around '=' in Apple's NVRAM files (the t2linux script strips them too)
        sed -E 's/^[[:space:]]*([^=[:space:]]+)[[:space:]]*=/\1=/' "$nv" | grep -v '^[[:space:]]*$' > "$dest/$base.txt"
        for c in "$chipdir/$platform-$antenna.clmb" "$chipdir/$platform.clmb"; do
            [ -f "$c" ] && { cp "$c" "$dest/$base.clm_blob"; break; }
        done
        for t in "$chipdir/$platform-$antenna.txcb" "$chipdir/$platform.txcb"; do
            [ -f "$t" ] && { cp "$t" "$dest/$base.txcap_blob"; break; }
        done
        echo "  $base"
        found=$((found + 1))
    done
done
[ "$found" -gt 0 ] || { echo "no BCM4364/BCM4377/BCM4355 Wi-Fi firmware under $src/wifi (is this a T2 Mac?)" >&2; exit 1; }
echo "wrote $found Wi-Fi firmware sets to $dest"

# ---- Bluetooth over PCIe (hci_bcm4377: BCM4377 in MacBookPro15,4, MacBookPro16,3, MacBookAir9,1) ------------------------
# The UART Bluetooth of the BCM4364 Macs (Mac mini 2018, MacBookPro15,1/15,2/15,3/16,1) runs from its ROM and needs no file.
# Apple's names look like  BCM4377B3_<...>_PCIE_macOS_<Board>_<...>_MUR_<...>.bin / .ptb ; Linux asks for
#   brcm/brcmbt<chip><stepping>-apple,<board>-<vendor>.bin / .ptb   (vendor MUR -> m, USI -> u, GEN -> none; DMI board
#   "apple,formosa" for the three Macs above). macOS Big Sur and older have no bluetooth folder; Monterey or later do.
bt=0
if [ -d "$src/bluetooth" ]; then
    mkdir -p "$dest/raw/bluetooth"
    for f in "$src"/bluetooth/*.bin "$src"/bluetooth/*.ptb; do
        [ -f "$f" ] || continue
        name=$(basename "$f"); ext=${name##*.}; root=${name%.*}
        case "$root" in *_DEV*) continue ;; esac                # Apple's development builds
        first=$(lower "${root%%_*}")
        printf '%s' "$first" | grep -Eq '^bcm43[0-9]{2}[a-z][0-9]$' || continue
        chip=$(printf '%s' "$first" | sed -E 's/^bcm(43[0-9]{2}).*/\1/')
        step=$(printf '%s' "$first" | sed -E 's/^bcm43[0-9]{2}//')
        # board type: the field after PCIE, or after PCIE_macOS; trailing E/S/2 characters dropped (Python rstrip("ES2"))
        board=$(printf '%s' "$root" | awk -F_ '{for(i=1;i<NF;i++) if($i=="PCIE"){ if($(i+1)=="macOS") print $(i+2); else print $(i+1); exit }}')
        [ -n "$board" ] || { echo "skip $name: no board type" >&2; continue; }
        board=$(lower "$(printf '%s' "$board" | sed -E 's/[ES2]+$//')")
        v=""; nv=0
        case "_${root}_" in *_MUR_*) v=m; nv=$((nv + 1)) ;; esac       # whole "_"-separated fields only
        case "_${root}_" in *_USI_*) v=u; nv=$((nv + 1)) ;; esac
        case "_${root}_" in *_GEN_*) v=""; nv=$((nv + 1)) ;; esac
        [ "$nv" -eq 1 ] || { echo "skip $name: vendor not clear" >&2; continue; }
        out="brcmbt${chip}${step}-apple,${board}"; [ -n "$v" ] && out="$out-$v"
        [ -f "$dest/$out.$ext" ] && { echo "skip $name: $out.$ext already written" >&2; continue; }
        cp "$f" "$dest/raw/bluetooth/"
        cp "$f" "$dest/$out.$ext"
        echo "  $out.$ext"
        bt=$((bt + 1))
    done
fi
# ---- Bluetooth over UART (hci_bcm: BCM4364 in the Mac mini 2018, MacBookPro15,1/15,2/15,3/16,1) -------------------------
# The chip runs from its ROM without a file (t2linux). If macOS has a patch-RAM file (.hcd) for it, Linux can load it:
# for a UART controller whose sub-version btbcm does not know (the BCM4364 is not in its table) the kernel asks for
# brcm/BCM.hcd - that is the name written here. Only when exactly one BCM4364 .hcd is found, so nothing is guessed.
hcd=0
if [ -d "$src/bluetooth" ]; then
    n=0; one=""
    for f in "$src"/bluetooth/BCM4364*.hcd "$src"/bluetooth/bcm4364*.hcd; do
        [ -f "$f" ] || continue
        n=$((n + 1)); one="$f"
    done
    if [ "$n" -eq 1 ]; then
        mkdir -p "$dest/raw/bluetooth"
        cp "$one" "$dest/raw/bluetooth/"
        cp "$one" "$dest/BCM.hcd"
        echo "  BCM.hcd (from $(basename "$one"))"
        hcd=1
    elif [ "$n" -gt 1 ]; then
        echo "found $n BCM4364 .hcd files in $src/bluetooth - not guessing which one; copied to $dest/raw/bluetooth" >&2
        mkdir -p "$dest/raw/bluetooth"
        for f in "$src"/bluetooth/BCM4364*.hcd "$src"/bluetooth/bcm4364*.hcd; do [ -f "$f" ] && cp "$f" "$dest/raw/bluetooth/"; done
    fi
fi
if [ "$hcd" -eq 0 ]; then
    echo "no BCM4364 UART Bluetooth patch file in macOS (normal: the chip starts from its ROM)"
fi
if [ "$bt" -gt 0 ]; then
    echo "wrote $bt Bluetooth firmware files (brcmbt*) to $dest"
else
    echo "no PCIe Bluetooth firmware copied (not needed for BCM4364 Macs; BCM4377 Macs need macOS Monterey or later)"
fi
echo "Mac mini 2018 needs: brcmfmac4364b2-pcie.apple,lanai-HRPN-u-7.5-X0.bin (+ .txt, .clm_blob)"
echo "The kernel log in IYAKKI names the exact files it looks for:  dmesg | grep -iE 'brcmfmac|bcm4377'"

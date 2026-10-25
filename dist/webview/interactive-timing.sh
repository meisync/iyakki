#!/bin/sh
# Measure what a page FEELS like in the full web engine (the one YouTube and Google use) while it is scrolled or plays a video:
# frames per second the engine's frame stream delivers, the largest gap between frames, how much of the picture each frame
# changed, the CPU of every engine process (WPEWebProcess, WPENetworkProcess, the helper, GPU process) and its memory.
#
#   sh interactive-timing.sh [options] [page...]
#     pages: google, youtube (a results page: a logged-out home page has no feed to scroll), watch, wikipedia, bbc (default: all five), or full https:// addresses (scrolled)
#     -s SECS      seconds of measuring per page (default 12)
#     -z WxH@PCT   picture size in device pixels and scale (default 2560x1400@100: the page area of a 2880x1800 panel, shown at 2560x1600)
#     -w MS        milliseconds between wheel notches while scrolling (default 25: a fast scroll of 40 notches a second)
#     -q QUALITY   watch page only: ask YouTube's player for a quality (small medium large hd720 hd1080...) before measuring;
#                  without it the player chooses (this is a page-script hack for measuring, not a product feature)
#     -v ID        watch page only: the YouTube video id (default aqz-KE-bpKQ, a 60 frames a second video offered up to 4K: the hardest
#                  public test; YE7VzlLtp-4 is a 30 frames a second one)
#     -e "K=V K=V" extra environment for the engine (to try WEBKIT_SKIA_* and other switches without a rebuild)
#     -p PROBE     the probe program (default /tmp/probe; build it with
#                  `cargo build --release -p iyakki-webview --example probe --target x86_64-unknown-linux-musl`,
#                  then copy it to the device: `ssh root@DEVICE 'cat > /tmp/probe && chmod +x /tmp/probe' < probe`)
#
# The probe is the shell's own webview client (crates/iyakki-webview), so the frame stream and the shared-file reads are the real
# ones; it starts a second engine with the same environment the shell gives the real one (webengine::helper_env), in a scratch
# profile under /tmp. Close the browser tile first, or the two engines share the CPU. The shell's own drawing cost (draw ms and
# screen write ms) is not in these numbers: it is in the shell's log line "shell: N frames in 10s, draw X ms, screen write Y ms"
# (`iyakki dev dmesg --grep frames` or the device's log) while the real tile is scrolled; see docs/BROWSER-SPEED.md.
# Nothing is written outside /tmp and /run.
SECS=12
SIZE=2560x1400
SCALE=100
EXTRA=""
WHEEL=25
QUAL=""
VID=aqz-KE-bpKQ
PROBE=/tmp/probe
while getopts s:z:e:p:w:q:v: o; do
  case $o in
    s) SECS=$OPTARG ;;
    z) SIZE=${OPTARG%@*}; case $OPTARG in *@*) SCALE=${OPTARG#*@} ;; esac ;;
    e) EXTRA="$EXTRA $OPTARG" ;;
    p) PROBE=$OPTARG ;;
    w) WHEEL=$OPTARG ;;
    q) QUAL=$OPTARG ;;
    v) VID=$OPTARG ;;
    *) exit 2 ;;
  esac
done
shift $((OPTIND - 1))
[ $# -eq 0 ] && set -- google youtube watch wikipedia bbc
HELPER=/usr/libexec/iyakki/webview-helper
[ -x "$PROBE" ] || { echo "no probe at $PROBE (see the header of this script)"; exit 2; }
[ -x "$HELPER" ] || HELPER=${IYAKKI_WEBVIEW_HELPER:-}
R=/run/iyakki-ibrowse
P=/tmp/ibrowse-profile
mkdir -p "$R" "$P/cache" "$P/config" "$P/data" && chmod 700 "$R"
ENVARGS="--env PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin --env HOME=$P --env XDG_RUNTIME_DIR=$R --env XDG_DATA_HOME=$P/data --env XDG_CONFIG_HOME=$P/config --env XDG_CACHE_HOME=$P/cache --env LANG=C.UTF-8 --env GST_REGISTRY_1_0=$P/cache/gst-registry.bin --env XKB_DEFAULT_LAYOUT=us"
# an Intel GPU without an Intel driver in Mesa: the shell forces llvmpipe (webengine::force_software_gl)
if grep -qi 0x8086 /sys/class/drm/card*/device/vendor 2>/dev/null && [ "$(cat /usr/share/iyakki/web-gl-drivers 2>/dev/null)" != intel ]; then
  ENVARGS="$ENVARGS --env LIBGL_ALWAYS_SOFTWARE=1"
fi
for kv in $EXTRA; do ENVARGS="$ENVARGS --env $kv"; done
echo "# $(date -u +%FT%TZ) size $SIZE scale $SCALE extra [$EXTRA] cpu0 $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq 2>/dev/null) kHz $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null) load $(cut -d' ' -f1-3 /proc/loadavg)"
for page in "$@"; do
  # steps after the load: settle, then idle 4 s (what the page costs when nothing is touched), then the interaction
  # the probe takes one step per word: the scripts below have no spaces
  JS_Q='JSON.stringify((function(v,q){if(!v)return"none";q=v.getVideoPlaybackQuality();return{w:v.videoWidth,h:v.videoHeight,t:v.currentTime,paused:v.paused,total:q.totalVideoFrames,dropped:q.droppedVideoFrames}})(document.querySelector("video")))'
  JS_PLAY='(function(v){if(v){v.muted=true;v.play()}return!0})(document.querySelector("video"))'
  JS_SETQ="(function(p){if(p&&p.setPlaybackQualityRange)p.setPlaybackQualityRange(\"$QUAL\",\"$QUAL\");return!0})(document.getElementById(\"movie_player\"))"
  SETQ=""
  [ -n "$QUAL" ] && SETQ="eval:$JS_SETQ wait:9"
  case $page in
    google)    url="https://www.google.com/search?q=rust+programming+language"; steps="wait:9 measure:4 measure:$SECS,120,$WHEEL" ;;
    youtube)   url="https://www.youtube.com/results?search_query=rust+programming"; steps="wait:12 measure:4 measure:$SECS,120,$WHEEL" ;;
    watch)     url="https://www.youtube.com/watch?v=$VID"
               steps="wait:14 eval:$JS_PLAY wait:6 $SETQ eval:$JS_Q measure:$SECS eval:$JS_Q" ;;
    wikipedia) url="https://en.wikipedia.org/wiki/Rust_(programming_language)"; steps="wait:8 measure:4 measure:$SECS,120,$WHEEL" ;;
    bbc)       url="https://www.bbc.com/news"; steps="wait:9 measure:4 measure:$SECS,120,$WHEEL" ;;
    http*)     url=$page; steps="wait:9 measure:4 measure:$SECS,120,$WHEEL" ;;
    *) echo "unknown page $page"; continue ;;
  esac
  echo "=== $page $url"
  # shellcheck disable=SC2086
  "$PROBE" "$HELPER" "$url" --size "$SIZE" --scale "$SCALE" --profile "$P" --allow '*' $ENVARGS $steps 2>&1 | grep -E "measure:|cpu |threads|script|GONE|^> measure|ready"
  echo "mem: $(awk '/MemAvailable/ {printf "%d MB available", $2/1024}' /proc/meminfo); cpu0 $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq 2>/dev/null) kHz"
done
rm -rf "$P" "$R"

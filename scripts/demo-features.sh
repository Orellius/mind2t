#!/bin/sh
# A one-screen tour of what the terminal can do -- run it inside RUUAH VT:
#   sh ~/Desktop/Studio/tools/ruuah-vt/scripts/demo-features.sh
# Written 2026-07-30, the night these features landed.

printf '\033[1m RUUAH VT feature tour \033[0m\n\n'

# --- styled underlines (SGR 4:x) + underline color (SGR 58) ---------------------
printf ' \033[4msingle\033[0m  \033[4:2mdouble\033[0m  \033[4:3m\033[58;2;255;80;80mcurly\033[0m  '
printf '\033[4:4m\033[58;2;80;180;255mdotted\033[0m  \033[4:5m\033[58;2;120;255;120mdashed\033[0m\n\n'

# --- true color (in since slice 1; tonight it just gets shown off) --------------
printf ' '
i=0
while [ $i -lt 60 ]; do
    r=$((255 - i * 4)); g=$((i * 4)); b=$((120 + i * 2))
    printf '\033[48;2;%d;%d;%dm ' "$r" "$g" "$b"
    i=$((i + 1))
done
printf '\033[0m\n\n'

# --- OSC 8 hyperlink: cmd+click the text below ----------------------------------
printf ' cmd+click: \033]8;;https://github.com/Orellius/ruuah-vt\007\033[4:1m\033[58;2;43;217;159mthe ruuah-vt repo\033[0m\033]8;;\007\n\n'

# --- VS16 emoji presentation + color emoji + Hebrew (bidi) ----------------------
printf ' emoji: \xf0\x9f\xa7\xa0 \xe2\x9d\xa4\xef\xb8\x8f \xf0\x9f\x8e\xa8   hebrew: '
printf '\xd7\xa9\xd7\x9c\xd7\x95\xd7\x9d \xd7\xa2\xd7\x95\xd7\x9c\xd7\x9d\n\n'

# --- kitty graphics: a gradient tile transmitted inline (f=32, chunked) ---------
printf ' kitty graphics: '
python3 - <<'PY'
import base64, sys
W = H = 96
px = bytearray()
for y in range(H):
    for x in range(W):
        px += bytes((255 * x // W, 90 + 165 * y // H, 217, 255))
data = base64.b64encode(bytes(px)).decode()
first = True
while data:
    chunk, data = data[:4000], data[4000:]
    keys = f"a=T,f=32,s={W},v={H},i=77,c=6,r=3,q=2,m={1 if data else 0}" if first \
        else f"m={1 if data else 0}"
    sys.stdout.write(f"\033_G{keys};{chunk}\033\\")
    first = False
sys.stdout.flush()
PY
printf '\n\n\n\n'

# --- sixel: three color bars through the OTHER image protocol -------------------
printf ' sixel: \033P0;0;0q"1;1;90;18#1;2;85;27;62#2;2;17;85;62#3;2;27;47;85'
printf '#1!30~-#2!30~-#3!30~\033\\\n\n\n\n'

# --- OSC 52: this script just wrote to your clipboard ---------------------------
printf '\033]52;c;UlVVQUggd2FzIGhlcmUgLSBwYXN0ZSBtZSE=\007'
printf ' clipboard: written via OSC 52 -- cmd+V somewhere to see it\n'

# --- OSC 777: a real macOS notification -----------------------------------------
printf '\033]777;notify;RUUAH VT;the demo script says hi\007'
printf ' notification: sent (first run asks permission)\n\n'

printf ' zoom: cmd+= / cmd+- / cmd+0    new tab: cmd+T    close: cmd+W\n'

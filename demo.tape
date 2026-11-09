# pzyp demo — render with:
#
#   cargo build --release
#   PZYP_DEMO_PW='your sudo password' vhs demo.tape
#
# The password is only used by a hidden `sudo -v` so the transactions don't
# prompt on camera. The demo really installs and then removes `cowsay`.

Output assets/demo.gif

Require ./target/release/pzyp

Set Shell "bash"
Set FontFamily "MonoLisa Nerd Font"
Set FontSize 16
Set Width 1400
Set Height 800
Set Padding 24
Set WindowBar Colorful
Set BorderRadius 10
Set TypingSpeed 60ms
Set Theme { "name": "vague", "black": "#252530", "red": "#d8647e", "green": "#7fa563", "yellow": "#f3be7c", "blue": "#6e94b2", "magenta": "#bb9dbd", "cyan": "#aeaed1", "white": "#cdcdcd", "brightBlack": "#606079", "brightRed": "#e08398", "brightGreen": "#99b782", "brightYellow": "#f5cb96", "brightBlue": "#8ba9c1", "brightMagenta": "#c9b1ca", "brightCyan": "#bebeda", "brightWhite": "#d7d7d7", "background": "#141415", "foreground": "#cdcdcd", "selection": "#252530", "cursor": "#cdcdcd" }

Hide
Type `export PATH="$PWD/target/release:$PATH" PS1='\[\e[34m\]$\[\e[0m\] ' PZYP_STYLE=uv`
Enter
Type `sudo -S -v <<< "$PZYP_DEMO_PW" 2>/dev/null; unset PZYP_DEMO_PW`
Enter
Type "sudo zypper --quiet --non-interactive remove cowsay >/dev/null 2>&1; clear"
Enter
Sleep 2s
Show

# Search and info
Type "pzyp search cowsay"
Sleep 400ms
Enter
Sleep 3s

Type "pzyp info cowsay"
Sleep 400ms
Enter
Sleep 4s

Type "clear"
Enter
Sleep 300ms

# Install, uv style (the default)
Type "pzyp add cowsay"
Sleep 400ms
Enter
Sleep 3s
Enter
Sleep 5s

Type "cowsay moo"
Enter
Sleep 2.5s

Type "clear"
Enter
Sleep 300ms

# Remove, pacman style
Type "pzyp --style pacman rm cowsay"
Sleep 400ms
Enter
Sleep 3s
Enter
Sleep 4s

Type "clear"
Enter
Sleep 300ms

# Same plan in the other styles, without touching anything
Type "pzyp --style dnf add -D cowsay"
Sleep 400ms
Enter
Sleep 3.5s

Type "pzyp --style apt add -D cowsay"
Sleep 400ms
Enter
Sleep 3.5s

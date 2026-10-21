# Dorker — Quick Start

From a fresh Windows PC to scanning in five steps. You type commands only in
steps 1–3; after that it's all buttons.

## 1. Install WSL  (one time, Windows)
Open **PowerShell as Administrator**, run this, then restart. When Ubuntu
opens, pick a username and password.
```
wsl --install
```

## 2. Get the Dorker folder
In the **Ubuntu** window, put the folder in your WSL home:
```
cp /mnt/c/Users/<you>/Downloads/dorker-linux-x86_64.tar.gz ~/
tar xzf ~/dorker-linux-x86_64.tar.gz -C ~/
```

## 3. Run the installer
Sets everything up and offers Rust + setup. Enter = No is fine for prompts.
```
cd ~/dorker && ./install.sh
```

## 4. Open the control panel
Starts Dorker and opens it in your browser (or double-click `Dorker-Panel.bat`
from Windows).
```
cd ~/dorker && ./panel.sh
```

## 5. Use it — all buttons
- **Setup**: paste your Google key → Save (for auto-discovery; optional).
- **Start**: find & scan. Or paste a link into **Scan one site**.
- Click a result to read its full report.
- **Stop** / **Quit panel** when done.
- **"Stop after N positives"** box: collect a set batch, then it stops.

## What the scores mean
```
 1–25  Very unlikely      76–85  Likely
26–50  Unlikely           86–90  Very likely
51–75  Possible           91–100 Almost certain
```

For authorized anti-fraud research. Dorker reads public pages politely — it
never logs in, attacks, or submits anything.

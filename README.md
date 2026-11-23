# purrdr.host

[繁體中文](#繁體中文) · [English](#english)

## 繁體中文

Purrdr 主機服務裝在你的電腦上，讓 iPhone／iPad 上的 [Purrdr](https://github.com/raychiutw/purrdr.ios) app 隨時連回這台電腦，查看並操作 [herdr](https://herdr.dev) 裡的 coding agent。

### 主要功能

- **手機操作 Agent**:在 app 上看 Agent 的畫面、跟它對話、上傳圖片與檔案。
- **推播通知**:Agent 需要你或做完時，手機會收到通知。
- **安全連線**:透過 Tailscale 以 HTTPS 連線，只有配對過的裝置能使用。
- **自動更新**:有新版會自動更新，不用手動處理。
- 支援 macOS、Linux、Windows。

### 安裝

先安裝並登入 [Tailscale](https://tailscale.com),再在終端機執行：

macOS／Linux:

```sh
curl -fsSL https://github.com/raychiutw/purrdr.host/releases/latest/download/install.sh | sh
```

Windows(PowerShell):

```powershell
irm https://github.com/raychiutw/purrdr.host/releases/latest/download/install.ps1 | iex
```

裝好後重開終端機，執行下面的指令，再用 app 掃描畫面上的 QR code 就完成配對：

```sh
purrdr pair
```

完整說明：https://raychiutw.github.io/purrdr/host-service/ · 安裝參數總表：https://raychiutw.github.io/purrdr/host-service/#install-options

## English

The Purrdr host service runs on your computer and lets the [Purrdr](https://github.com/raychiutw/purrdr.ios) app on your iPhone or iPad connect back to it, so you can watch and work with the coding agents in [herdr](https://herdr.dev).

### Features

- **Agents from your phone**: see what an agent is doing, chat with it, and upload images and files.
- **Push notifications**: get notified when an agent needs you or finishes.
- **Secure connection**: HTTPS over Tailscale; only paired devices can connect.
- **Automatic updates**: new versions install themselves.
- Runs on macOS, Linux and Windows.

### Install

Install and sign in to [Tailscale](https://tailscale.com) first, then run this in a terminal:

macOS / Linux:

```sh
curl -fsSL https://github.com/raychiutw/purrdr.host/releases/latest/download/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://github.com/raychiutw/purrdr.host/releases/latest/download/install.ps1 | iex
```

Then open a new terminal, run the command below, and scan the QR code with the app to pair:

```sh
purrdr pair
```

Full guide: https://raychiutw.github.io/purrdr/host-service/ · All install options: https://raychiutw.github.io/purrdr/host-service/#install-options

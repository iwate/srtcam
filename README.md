# rstcam

Linux 向けの SRT 受信サーバーです。受信した映像を v4l2loopback デバイスへ流し込みます。

このアプリは、送信側が未接続でも映像出力を止めません。
起動直後・切断時・受信停止時はダミー画像を連続送出し、デバイスを常に Capture として認識させ続けます。

## できること

- SRT を LISTEN モードで待受（既定: 5000）
- 受信映像を /dev/video10（設定可）へ出力
- 音声は無視（映像のみ）
- ライブ映像が無い間はダミー画像を出力
- 以下の設定項目を変更可能
	- listen_port
	- srt_latency_ms
	- loopback_device
	- dummy_image

## 前提条件

1. Linux で v4l2loopback が使えること

2. ループバックデバイスを作成

```bash
sudo modprobe v4l2loopback video_nr=10 card_label=rstcam exclusive_caps=1
```

3. デバイス確認

```bash
ls -l /dev/video10
```

4. ffmpeg（SRT対応ビルド）を使用可能にする

5. Nix を使う場合はこのリポジトリで次を実行

```bash
nix develop
```

## 初回セットアップ

1. 設定ファイルを作成

```bash
cp config.example.toml config.toml
```

2. 必要なら config.toml を編集

最小変更例:

```toml
listen_port = 5000
srt_latency_ms = 120
loopback_device = "/dev/video10"
dummy_image = "dummy.png"
```

## 起動方法

### Nix 環境で起動（推奨）

```bash
nix develop -c cargo run -- --config config.toml
```

### 直接起動

```bash
cargo run -- --config config.toml
```

## 動作確認手順

1. 受信側として rstcam を起動

2. 別ターミナルで送信開始

```bash
ffmpeg -re -stream_loop -1 -i input.mp4 -an -c:v libx264 -f mpegts "srt://127.0.0.1:5000?pkt_size=1316"
```

3. さらに別ターミナルでループバックを表示

```bash
ffplay -f v4l2 /dev/video10
```

## 期待される挙動

1. 送信前
- /dev/video10 にはダミー画像が表示され続ける

2. 送信中
- ダミー画像からライブ映像へ切り替わる

3. 送信停止時
- ライブ映像からダミー画像へ自動復帰する
- アプリは終了せず待受継続する

## 主な CLI オプション

```bash
rstcam --config config.toml \
	--listen-port 5000 \
	--srt-latency-ms 120 \
	--loopback-device /dev/video10 \
	--dummy-image dummy.png
```

設定ファイル値より CLI 値が優先されます。

## トラブルシュート

1. /dev/video10 が無い
- v4l2loopback の modprobe が成功しているか確認

2. 映像が出ない
- ffmpeg が SRT 対応か確認
- 送信側 URL と listen_port が一致しているか確認

3. 権限エラー
- /dev/video10 の書き込み権限を確認
- 必要に応じて video グループへユーザーを追加

4. CPU 使用率が高い
- まず解像度や fps を下げる
- 将来的に AMD Radeon の VA-API 経路を追加可能

## 補足

- 現在は安定動作優先で CPU デコード経路を採用
- VA-API は必要時にオプションとして追加する想定

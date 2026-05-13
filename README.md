# rstcam

Linux 向けの SRT 受信サーバーです。受信した映像を v4l2loopback デバイスへ流し込みます。

送信側が未接続でも映像出力を止めず、起動直後・切断時・受信停止時はダミー画像を連続送出します。これにより、デバイスを常に Capture として認識させ続けます。

## できること

- SRT を LISTEN モードで待受（既定: 5000）
- 受信映像を /dev/video10（設定可）へ出力
- 音声は無視（映像のみ）
- ライブ映像が無い間はダミー画像を出力
- フレームサイズを固定パラメータで指定
- `analyzeduration=0` を設定で利用可能（既定値 0）

## 前提条件

1. Linux で v4l2loopback が使えること
2. FFmpeg ライブラリ（libavformat/libavcodec など）が利用可能であること
3. デバイスへ書き込み可能であること

v4l2loopback 作成例:

```bash
sudo modprobe v4l2loopback video_nr=10 card_label=rstcam exclusive_caps=1
```

確認:

```bash
ls -l /dev/video10
```

## セットアップ

設定ファイルを作成:

```bash
cp config.example.toml config.toml
```

最小設定例（HD固定）:

```toml
listen_port = 5000
srt_latency_ms = 120
latency_profile = "balanced"
loopback_device = "/dev/video10"
dummy_image = "dummy.png"
frame_width = 1280
frame_height = 720
fps = 30
ffmpeg_analyzeduration_us = 0
ffmpeg_probesize_bytes = 32768
```

低遅延寄り設定例:

```toml
latency_profile = "ultra-low"
srt_latency_ms = 30
live_timeout_ms = 300
live_channel_capacity = 1
ffmpeg_analyzeduration_us = 0
ffmpeg_probesize_bytes = 32768
```

## 起動

Nix 環境で起動（推奨）:

```bash
nix develop -c cargo run -- --config config.toml
```

直接起動:

```bash
cargo run -- --config config.toml
```

## 動作確認

1. 受信側として rstcam を起動
2. 別ターミナルで送信開始

```bash
ffmpeg -re -stream_loop -1 -i input.mp4 -an -c:v libx264 -f mpegts "srt://127.0.0.1:5000?pkt_size=1316"
```

3. さらに別ターミナルでループバック表示

```bash
ffplay -f v4l2 /dev/video10
```

## 期待される挙動

1. 送信前: ダミー画像を表示し続ける
2. 送信中: ダミーからライブ映像へ切り替わる
3. 送信停止時: ライブからダミーへ自動復帰し、待受継続

## 主な設定項目

- `listen_port`: SRT LISTEN ポート
- `srt_latency_ms`: SRT レイテンシ
- `frame_width`, `frame_height`: 固定フレームサイズ（既定 HD）
- `fps`: 出力 FPS
- `loopback_device`: ループバックデバイス
- `dummy_image`: ダミー画像パス
- `ffmpeg_analyzeduration_us`: ffmpeg の analyzeduration
- `ffmpeg_probesize_bytes`: ffmpeg の probesize

CLI は設定ファイルより優先されます。

## トラブルシュート

1. /dev/video10 が無い
- v4l2loopback の modprobe が成功しているか確認

2. 映像が出ない
- ffmpeg が SRT 対応か確認
- 送信側 URL と `listen_port` が一致しているか確認
- 入力映像と `frame_width`/`frame_height` の差が大きい場合、まず HD で確認

3. 権限エラー
- /dev/video10 の書き込み権限を確認
- 必要に応じて video グループへユーザー追加

4. 遅延が大きい
- `latency_profile = "ultra-low"` を使用
- `srt_latency_ms` を 20-40 で調整
- 送信側を `-tune zerolatency -bf 0` で低遅延化

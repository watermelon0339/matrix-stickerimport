#!/bin/bash

set -euo pipefail

# 用法:
#   ./to-webm.sh <输入目录> <输出目录> [CRF=32] [FPS=30]
# 示例:
#   ./to-webm.sh ./inputs ./outputs 32 30
# 说明:
#   - 输出文件名与输入文件名保持一致，仅扩展名改为 .webm
#   - GIF：按给定 FPS 正常转换
#   - PNG：转换为 1 帧的 webm（静态一帧）

INDIR=${1:-}
OUTDIR=${2:-}
CRF=${3:-32}
FPS=${4:-30}
SIZE=512   # 最长边限制为 512 像素

# 参数检查
if [[ -z "$INDIR" || -z "$OUTDIR" ]]; then
    echo "错误：请提供 输入目录 和 输出目录"
    echo "用法：$(basename "$0") <输入目录> <输出目录> [CRF=32] [FPS=30]"
    exit 1
fi

if [[ ! -d "$INDIR" ]]; then
    echo "错误：输入目录不存在：$INDIR"
    exit 1
fi

mkdir -p "$OUTDIR"

echo "输入目录: $INDIR"
echo "输出目录: $OUTDIR"
echo "CRF: $CRF"
echo "FPS: $FPS"
echo "开始批量转换..."
echo "-----------------------------------"

# 统一的缩放滤镜：最长边 512，保持纵横比
SCALE_FILTER="scale=if(gt(a\\,1)\\,${SIZE}\\,-2):if(gt(a\\,1)\\,-2\\,${SIZE}):flags=lanczos"

shopt -s nullglob

for path in "$INDIR"/*.gif "$INDIR"/*.GIF "$INDIR"/*.png "$INDIR"/*.PNG; do
    [[ -e "$path" ]] || continue

    filename=$(basename -- "$path")
    name_noext="${filename%.*}"
    out="$OUTDIR/$name_noext.webm"

    # 获取小写扩展名
    ext="${filename##*.}"
    ext_lc="${ext,,}"

    case "$ext_lc" in
        gif)
            echo "转换 GIF：$path → $out"
            ffmpeg -y -i "$path" \
                -vf "${SCALE_FILTER},fps=${FPS}" \
                -c:v libvpx-vp9 -b:v 0 -crf ${CRF} \
                -an "$out"
            ;;
        png)
            echo "转换 PNG 为 1 帧 webm：$path → $out"
            ffmpeg -y -i "$path" \
                -vf "${SCALE_FILTER}" \
                -c:v libvpx-vp9 -b:v 0 -crf ${CRF} \
                -an -frames:v 1 "$out"
            ;;
        *)
            # 不应命中，保留
            continue
            ;;
    esac

    echo "完成：$out"
    echo "-----------------------------------"
done

echo "全部转换完成"


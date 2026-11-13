param(
    [string]$Source = "assets/icon.png",
    [string]$Output = "assets/icon.ico",
    [double]$Padding = 0.08,
    [switch]$NoFit
)

# Builds a multi-size .ico (16..256 px) from a square PNG.
#
# Unless -NoFit is given, the transparent margins of the source are trimmed and
# the artwork is re-centred with `-Padding` (fraction of the icon) so the mark
# stays readable at 16/24/32 px.
#
# Usage: powershell -ExecutionPolicy Bypass -File tools/make-icon.ps1 [-Padding 0.08] [-NoFit]

Add-Type -AssemblyName System.Drawing

Add-Type -TypeDefinition @"
using System;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;

public static class IconFit
{
    // Trims transparent margins and re-centres the artwork on a square canvas.
    public static void Normalize(string source, string target, float padding)
    {
        using (var bitmap = new Bitmap(source))
        {
            int minX = bitmap.Width, minY = bitmap.Height, maxX = -1, maxY = -1;
            var data = bitmap.LockBits(
                new Rectangle(0, 0, bitmap.Width, bitmap.Height),
                ImageLockMode.ReadOnly,
                PixelFormat.Format32bppArgb);
            var buffer = new byte[data.Stride * bitmap.Height];
            Marshal.Copy(data.Scan0, buffer, 0, buffer.Length);
            bitmap.UnlockBits(data);

            int stride = data.Stride;
            for (int y = 0; y < bitmap.Height; y++)
            {
                int row = y * stride;
                for (int x = 0; x < bitmap.Width; x++)
                {
                    if (buffer[row + x * 4 + 3] > 8)
                    {
                        if (x < minX) minX = x;
                        if (x > maxX) maxX = x;
                        if (y < minY) minY = y;
                        if (y > maxY) maxY = y;
                    }
                }
            }

            if (maxX < 0 || maxY < 0)
            {
                minX = 0; minY = 0; maxX = bitmap.Width - 1; maxY = bitmap.Height - 1;
            }

            var srcRect = new Rectangle(minX, minY, maxX - minX + 1, maxY - minY + 1);
            int size = 1024;
            int inner = Math.Max(16, (int)Math.Round(size * (1.0 - 2.0 * padding)));

            using (var canvas = new Bitmap(size, size, PixelFormat.Format32bppArgb))
            using (var graphics = Graphics.FromImage(canvas))
            {
                graphics.SmoothingMode = SmoothingMode.HighQuality;
                graphics.InterpolationMode = InterpolationMode.HighQualityBicubic;
                graphics.PixelOffsetMode = PixelOffsetMode.HighQuality;
                graphics.Clear(Color.Transparent);

                double scale = Math.Min((double)inner / srcRect.Width, (double)inner / srcRect.Height);
                int width = Math.Max(1, (int)Math.Round(srcRect.Width * scale));
                int height = Math.Max(1, (int)Math.Round(srcRect.Height * scale));

                graphics.DrawImage(
                    bitmap,
                    new Rectangle((size - width) / 2, (size - height) / 2, width, height),
                    srcRect,
                    GraphicsUnit.Pixel);

                canvas.Save(target, ImageFormat.Png);
            }
        }
    }
}
"@ -ReferencedAssemblies System.Drawing

if (-not (Test-Path $Source)) {
    throw "Source image not found: $Source"
}

$resolved = (Resolve-Path $Source).Path
if (-not $NoFit) {
    $fitted = Join-Path ([System.IO.Path]::GetTempPath()) "br1-player-icon-fitted.png"
    [IconFit]::Normalize($resolved, $fitted, [float]$Padding)
    $resolved = $fitted
}

$sizes = [int[]](16, 24, 32, 48, 64, 128, 256)
$image = [System.Drawing.Image]::FromFile($resolved)

$frames = New-Object System.Collections.Generic.List[byte[]]
foreach ($size in $sizes) {
    $bitmap = New-Object System.Drawing.Bitmap $size, $size
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $graphics.Clear([System.Drawing.Color]::Transparent)
    $graphics.DrawImage($image, 0, 0, $size, $size)
    $graphics.Dispose()

    $stream = New-Object System.IO.MemoryStream
    $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    $frames.Add($stream.ToArray())
    $stream.Dispose()
    $bitmap.Dispose()
}
$image.Dispose()

$outputPath = Join-Path (Get-Location) $Output
$fs = [System.IO.File]::Create($outputPath)
$writer = New-Object System.IO.BinaryWriter($fs)

# ICONDIR
$writer.Write([uint16]0)
$writer.Write([uint16]1)
$writer.Write([uint16]$frames.Count)

$offset = 6 + 16 * $frames.Count
for ($index = 0; $index -lt $frames.Count; $index++) {
    $size = $sizes[$index]
    $dimension = if ($size -ge 256) { 0 } else { $size }

    # ICONDIRENTRY
    $writer.Write([byte]$dimension)
    $writer.Write([byte]$dimension)
    $writer.Write([byte]0)
    $writer.Write([byte]0)
    $writer.Write([uint16]1)
    $writer.Write([uint16]32)
    $writer.Write([uint32]$frames[$index].Length)
    $writer.Write([uint32]$offset)

    $offset += $frames[$index].Length
}

foreach ($frame in $frames) {
    $writer.Write($frame)
}

$writer.Close()
$fs.Close()

Write-Output "Wrote $Output from $Source"

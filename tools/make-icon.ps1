param(
    [string]$Source = "assets/icon.png",
    [string]$Output = "assets/icon.ico"
)

# Builds a multi-size .ico (16..256 px) from a square PNG.
# Usage: powershell -ExecutionPolicy Bypass -File tools/make-icon.ps1

Add-Type -AssemblyName System.Drawing

if (-not (Test-Path $Source)) {
    throw "Source image not found: $Source"
}

$sizes = [int[]](16, 24, 32, 48, 64, 128, 256)
$image = [System.Drawing.Image]::FromFile((Resolve-Path $Source))

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

$fs = [System.IO.File]::Create((Join-Path (Get-Location) $Output))
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

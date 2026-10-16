# Rebuild the vector-drawn application icon; no external image dependencies.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$sizes = @(16, 20, 24, 32, 48, 64, 128, 256)
$frames = @()
foreach ($size in $sizes) {
    $bitmap = [Drawing.Bitmap]::new($size, $size)
    $g = [Drawing.Graphics]::FromImage($bitmap)
    $g.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.ScaleTransform($size / 64.0, $size / 64.0)
    $background = [Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#122338'))
    $bars = [Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#36D6B0'))
    $line = [Drawing.Pen]::new([Drawing.Color]::White, 4)
    $line.StartCap = $line.EndCap = [Drawing.Drawing2D.LineCap]::Round
    $line.LineJoin = [Drawing.Drawing2D.LineJoin]::Round
    $shape = [Drawing.Drawing2D.GraphicsPath]::new()
    $shape.AddArc(2, 2, 20, 20, 180, 90)
    $shape.AddArc(42, 2, 20, 20, 270, 90)
    $shape.AddArc(42, 42, 20, 20, 0, 90)
    $shape.AddArc(2, 42, 20, 20, 90, 90)
    $shape.CloseFigure()
    $g.FillPath($background, $shape)
    $g.FillRectangle($bars, 12, 40, 9, 12)
    $g.FillRectangle($bars, 28, 33, 9, 19)
    $g.FillRectangle($bars, 44, 26, 9, 26)
    $points = [Drawing.PointF[]]@([Drawing.PointF]::new(13,32), [Drawing.PointF]::new(27,22), [Drawing.PointF]::new(36,27), [Drawing.PointF]::new(50,13))
    $g.DrawLines($line, $points)
    $g.DrawLine($line, 41, 13, 50, 13)
    $g.DrawLine($line, 50, 13, 50, 22)
    $stream = [IO.MemoryStream]::new()
    $bitmap.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
    $frames += ,$stream.ToArray()
    $stream.Dispose(); $shape.Dispose(); $line.Dispose(); $bars.Dispose(); $background.Dispose(); $g.Dispose(); $bitmap.Dispose()
}
$output = [IO.File]::Create((Join-Path $PSScriptRoot 'app.ico'))
$writer = [IO.BinaryWriter]::new($output)
try {
    $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$sizes.Count)
    $offset = 6 + 16 * $sizes.Count
    for ($i = 0; $i -lt $sizes.Count; $i++) {
        $dimension = if ($sizes[$i] -eq 256) { 0 } else { $sizes[$i] }
        $writer.Write([byte]$dimension); $writer.Write([byte]$dimension)
        $writer.Write([byte]0); $writer.Write([byte]0)
        $writer.Write([uint16]1); $writer.Write([uint16]32)
        $writer.Write([uint32]$frames[$i].Length); $writer.Write([uint32]$offset)
        $offset += $frames[$i].Length
    }
    foreach ($frame in $frames) { $writer.Write([byte[]]$frame) }
} finally { $writer.Dispose() }

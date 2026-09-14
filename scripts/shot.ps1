# 截取屏幕底部一条区域（只看 dock），避免拍到桌面上其他窗口
param(
  [string]$Out = "C:\UI_desktop\shot.png",
  [int]$Height = 620
)

Add-Type -AssemblyName System.Windows.Forms, System.Drawing

$b = [Windows.Forms.Screen]::PrimaryScreen.Bounds
$h = [Math]::Min($Height, $b.Height)
$x = $b.X
$y = $b.Y + $b.Height - $h

$bmp = New-Object Drawing.Bitmap $b.Width, $h
$gfx = [Drawing.Graphics]::FromImage($bmp)
$gfx.CopyFromScreen($x, $y, 0, 0, (New-Object Drawing.Size $b.Width, $h))
$bmp.Save($Out, [Drawing.Imaging.ImageFormat]::Png)
$gfx.Dispose()
$bmp.Dispose()

Write-Output "saved $Out ($($b.Width)x$h from y=$y)"

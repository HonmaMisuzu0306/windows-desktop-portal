@echo off
REM 安装 MSVC C++ 生成工具（Tauri 在 Windows 编译 Rust 后端必需）
REM 需要管理员权限：右键“以管理员身份运行”
echo Installing MSVC Build Tools (VC++ workload)...
winget install --id Microsoft.VisualStudio.2022.BuildTools ^
  --accept-source-agreements ^
  --accept-package-agreements ^
  --disable-interactivity ^
  --override "--quiet --wait --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
echo.
echo Exit code: %ERRORLEVEL%

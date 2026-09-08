@echo off
setlocal EnableExtensions
rem ============================================================
rem  One-time helper: pre-populate the Tauri NSIS bundler cache.
rem
rem  `npx tauri build --bundles nsis` normally downloads NSIS from
rem  GitHub, which times out on slow/filtered networks (China).
rem  This script downloads the two files with curl (resume + retry)
rem  and places them where the bundler expects them, so the build
rem  proceeds without any GitHub download.
rem
rem  Usage: double-click. No admin rights needed.
rem
rem  Files (pinned to tauri-cli 2.11.4 / tauri-bundler):
rem    https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.11/nsis-3.11.zip
rem    https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll
rem ============================================================

set "TAURI=%LOCALAPPDATA%\tauri"
set "BASE=%TAURI%\x86_64-pc-windows-msvc"
if not exist "%BASE%" if exist "%TAURI%\nsis" set "BASE=%TAURI%"
set "NSIS_DIR=%BASE%\nsis"
set "WORK=%TEMP%\tauri-nsis-prep"

if exist "%WORK%" rmdir /s /q "%WORK%"
mkdir "%WORK%" || goto fail
cd /d "%WORK%"

echo ==^> [1/4] Downloading nsis-3.11.zip (this is the big one, ~10MB) ...
echo ==^>       Re-run this script to resume if it fails partway.
curl -L --retry 10 --retry-delay 5 --retry-all-errors --connect-timeout 30 -C - -o nsis-3.11.zip "https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.11/nsis-3.11.zip"
if errorlevel 1 goto fail_dl

echo ==^> [2/4] Downloading nsis_tauri_utils.dll ...
curl -L --retry 10 --retry-delay 5 --retry-all-errors --connect-timeout 30 -C - -o nsis_tauri_utils.dll "https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll"
if errorlevel 1 goto fail_dl

echo ==^> [3/4] Extracting ...
tar -xf nsis-3.11.zip || goto fail_extract
if exist "%WORK%\nsis-3.11\makensis.exe" set "SRC=%WORK%\nsis-3.11"
if not defined SRC if exist "%WORK%\makensis.exe" set "SRC=%WORK%"
if not defined SRC goto fail_extract

echo ==^> [4/4] Placing into Tauri cache: %NSIS_DIR%
if not exist "%NSIS_DIR%" mkdir "%NSIS_DIR%" || goto fail
robocopy "%SRC%" "%NSIS_DIR%" /E /NFL /NDL /NJH /NJS >nul
if errorlevel 8 goto fail_copy

if not exist "%NSIS_DIR%\Plugins\x86-unicode\additional" mkdir "%NSIS_DIR%\Plugins\x86-unicode\additional"
copy /y "%WORK%\nsis_tauri_utils.dll" "%NSIS_DIR%\Plugins\x86-unicode\additional\" >nul || goto fail
copy /y "%WORK%\nsis-3.11.zip" "%BASE%\" >nul 2>nul

if not exist "%NSIS_DIR%\makensis.exe" goto fail_done
echo.
echo SUCCESS: Tauri NSIS cache populated.
echo   NSIS dir: %NSIS_DIR%
echo Next step:  scripts\build-windows.bat
echo (the Rust build is already done - only the bundling step will run)
pause
exit /b 0

:fail_dl
echo.
echo ERROR: download failed (network problem - GitHub is often slow here).
echo Re-run this script to resume, or download the two URLs manually
echo in a browser / download manager and put them in: %WORK%
pause
exit /b 1

:fail_extract
echo.
echo ERROR: nsis-3.11.zip looks invalid or does not contain makensis.exe.
echo Delete the zip in %WORK% and re-run.
pause
exit /b 1

:fail_copy
echo.
echo ERROR: copying into the Tauri cache failed.
pause
exit /b 1

:fail_done
echo.
echo ERROR: expected layout not found after copy. Inspect %NSIS_DIR%
pause
exit /b 1

:fail
echo.
echo ERROR: could not create the work directory.
pause
exit /b 1

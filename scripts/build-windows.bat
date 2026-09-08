@echo off
setlocal EnableExtensions
rem ============================================================
rem  Build the Windows (x86_64-pc-windows-msvc) version of
rem  DSH Launcher natively on Windows.
rem
rem  (Cross-compiling from macOS is still available via
rem   scripts\build-windows.sh; this script is the native path.)
rem
rem  Usage:
rem    scripts\build-windows.bat             full release build + NSIS installer
rem    scripts\build-windows.bat check       fast type-check only (cargo check)
rem    scripts\build-windows.bat clean       remove target build artifacts
rem
rem  One-time prerequisites:
rem    1. Rust stable, MSVC host (default on Windows):
rem         winget install RustLang.Rustup        (or https://rustup.rs)
rem         - or the project-local .toolchain is used automatically
rem    2. C++ Build Tools with the "Desktop development with C++" workload:
rem         https://visualstudio.microsoft.com/zh-hans/products/visual-studio-build-tools/
rem    3. Node.js 20+  (https://nodejs.org)
rem    4. NSIS, for the installer bundle:  winget install NSIS.NSIS
rem
rem  Output:
rem    src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis\
rem    (the resulting -setup.exe is renamed so the file name has no spaces)
rem
rem  NOTE: written in flat goto style on purpose - set statements that
rem  expand %PATH% inside parenthesized blocks break cmd's parser.
rem ============================================================

set "PROJECT_ROOT=%~dp0.."
set "MODE=%1"
set "NSIS_DIR=%PROJECT_ROOT%\src-tauri\target\x86_64-pc-windows-msvc\release\bundle\nsis"
rem Expand the parenthesized Program Files names up front.
set "PF86=%ProgramFiles(x86)%"
set "PF=%ProgramFiles%"

cd /d "%PROJECT_ROOT%" || exit /b 1

rem --- Rust -----------------------------------------------------
rem Prefer the system cargo; fall back to the project-local toolchain
rem in .toolchain (workspace-local installs).
where cargo >nul 2>nul
if not errorlevel 1 goto rust_ok
if not exist "%PROJECT_ROOT%\.toolchain\cargo\bin\cargo.exe" goto rust_missing
set "RUSTUP_HOME=%PROJECT_ROOT%\.toolchain\rustup"
set "CARGO_HOME=%PROJECT_ROOT%\.toolchain\cargo"
set "PATH=%CARGO_HOME%\bin;%PATH%"
echo ==^> Using project-local Rust toolchain (.toolchain)
goto rust_ok
:rust_missing
echo ERROR: cargo not found in PATH or in .toolchain\cargo\bin.
echo Fix:   winget install RustLang.Rustup    (or install from https://rustup.rs)
exit /b 1
:rust_ok

rem --- Node.js --------------------------------------------------
where node >nul 2>nul
if not errorlevel 1 goto node_ok
echo ERROR: node/npm not found in PATH.
echo Fix:   install Node.js 20+ from https://nodejs.org
exit /b 1
:node_ok

rem --- NSIS (only needed for the installer bundle) --------------
if /I not "%MODE%"=="build" goto nsis_ok
where makensis >nul 2>nul
if not errorlevel 1 goto nsis_ok
if exist "%PF86%\NSIS\makensis.exe" goto nsis_pf86
if exist "%PF%\NSIS\makensis.exe" goto nsis_pf
echo ERROR: makensis (NSIS) not found in PATH.
echo Fix:   winget install NSIS.NSIS
echo        then make sure C:\Program Files (x86)\NSIS is on PATH.
exit /b 1
:nsis_pf86
set "PATH=%PF86%\NSIS;%PATH%"
goto nsis_ok
:nsis_pf
set "PATH=%PF%\NSIS;%PATH%"
goto nsis_ok
:nsis_ok

rem --- MSVC environment (linker + C build scripts) --------------
rem Locate the C++ Build Tools / Visual Studio installation and
rem activate its x64 developer environment for this script.
set "VSWHERE=%PF86%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" set "VSWHERE=%PF%\Microsoft Visual Studio\Installer\vswhere.exe"
set "VCVARS="
if not exist "%VSWHERE%" goto vs_missing
for /f "usebackq tokens=*" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VS_PATH=%%i"
if not defined VS_PATH goto vs_missing
if not exist "%VS_PATH%\VC\Auxiliary\Build\vcvars64.bat" goto vs_missing
set "VCVARS=%VS_PATH%\VC\Auxiliary\Build\vcvars64.bat"
goto vs_found
:vs_missing
if /I "%MODE%"=="check" goto vs_warn
echo ERROR: MSVC C++ Build Tools not found (no linker available).
echo Fix:   install the C++ Build Tools with the "Desktop development
echo        with C++" workload:
echo        https://visualstudio.microsoft.com/zh-hans/products/visual-studio-build-tools/
exit /b 1
:vs_warn
echo WARNING: C++ Build Tools not found. cargo check still needs the
echo          MSVC linker (link.exe) to compile dependency build scripts:
echo          install the C++ Build Tools, workload "Desktop development
echo          with C++".
goto vs_done
:vs_found
call "%VCVARS%" >nul
:vs_done

rem --- disk space ------------------------------------------------
for /f "delims=" %%f in ('powershell -NoProfile -Command "$d=(Get-Item '%PROJECT_ROOT%').PSDrive; if ([int64]($d.Free/1GB) -lt 10) { 'LOW' }" 2^>nul') do set "DISK=%%f"
if "%DISK%"=="LOW" echo WARNING: less than 10GB free disk; a Windows release build needs ~5-8GB.

rem --- dispatch ---------------------------------------------------
if /I "%MODE%"=="check" goto check
if /I "%MODE%"=="build" goto build
if /I "%MODE%"=="clean" goto clean
echo Usage: build-windows.bat [check^|build^|clean]
exit /b 2

:check
pushd src-tauri
call cargo check
set "RC=%ERRORLEVEL%"
popd
if not "%RC%"=="0" goto check_failed
echo ==^> Windows target type-check OK.
goto end
:check_failed
echo ERROR: cargo check failed.
exit /b %RC%

:build
if exist node_modules goto npm_ok
echo ==^> node_modules missing - running npm install ...
call npm install --no-audit --no-fund
if errorlevel 1 goto npm_failed
:npm_ok
call npx tauri build --bundles nsis
if errorlevel 1 goto build_failed
if exist "%NSIS_DIR%" for %%F in ("%NSIS_DIR%\DSH Launcher_*.exe") do call :rename_installer "%%F"
echo ==^> Build finished. Installer:
echo     %NSIS_DIR%\DSH_Launcher_*_x64-setup.exe
goto end
:npm_failed
echo ERROR: npm install failed.
exit /b 1
:build_failed
echo ERROR: tauri build failed.
exit /b 1

rem Rename one installer file: drop spaces from the name.
:rename_installer
set "BASE=%~nx1"
set "BASE=%BASE: =_%"
ren "%~1" "%BASE%"
echo ==^> Renamed installer: %~nx1 -^> %BASE%
goto :eof

:clean
if exist "%PROJECT_ROOT%\src-tauri\target" (
    rmdir /s /q "%PROJECT_ROOT%\src-tauri\target"
    echo ==^> Removed src-tauri\target
)
goto end

:end
endlocal
exit /b 0

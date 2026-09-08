@echo off
setlocal EnableExtensions
rem ============================================================
rem  One-time helper: install Visual Studio 2022 Build Tools with
rem  the "Desktop development with C++" workload (provides cl.exe,
rem  link.exe, CRT and the Windows SDK needed to build this
rem  project on Windows).
rem
rem  Usage: double-click it. If you are not already admin, the
rem  script re-launches ITSELF in an elevated window (approve the
rem  UAC prompt) - that elevated window then runs the whole
rem  install and stays open until it is done.
rem
rem  Re-running the script is always safe: the installer resumes
rem  where it left off.
rem
rem  After success, verify with:  scripts\build-windows.bat check
rem ============================================================
rem Normalize the path (no ".." - the installer rejects such paths with
rem exit code 87).
for %%I in ("%~dp0..\.toolchain") do set "TC=%%~fI"
set "BOOT=%TC%\vs_buildtools.exe"

rem --- require administrator: relaunch this script elevated ------
net session >nul 2>&1
if not errorlevel 1 goto admin
echo Requesting administrator rights - approve the UAC prompt that
echo appears. A second (elevated) window will run the install.
powershell -NoProfile -Command "Start-Process -LiteralPath '%~f0' -Verb RunAs"
echo.
echo If the UAC prompt was cancelled, nothing was installed.
pause
exit /b 1

:admin
if not exist "%BOOT%" (
    echo ==^> Downloading the official VS Build Tools bootstrapper ...
    curl -sSL --retry 3 -o "%BOOT%" https://aka.ms/vs/17/release/vs_buildtools.exe
    if errorlevel 1 (
        echo ERROR: bootstrapper download failed. Check the network
        echo        and re-run, or download manually:
        echo        https://aka.ms/vs/17/release/vs_buildtools.exe
        pause
        exit /b 1
    )
)

echo ==^> install-vsbuildtools v3 (no --log flag)
echo ==^> Running VS Build Tools installer (C++ workload).
echo ==^> This downloads several GB and takes 5-20 minutes.
echo ==^> Keep this window open; do not close it.
echo.

rem NOTE: no --log flag - the channel bootstrapper does not support it
rem (exit code 87). Standard logs are written to %TEMP%\dd_*.log.
"%BOOT%" --add Microsoft.VisualStudio.Workload.VCTools ^
    --quiet --wait --norestart --nocache
set "RC=%ERRORLEVEL%"

echo.
echo Installer exit code: %RC%
if not "%RC%"=="0" (
    echo ERROR: install reported failure.
    echo For details open %TEMP% and find the newest file named
    echo dd_*.log (or dd_setup*.log).
    echo Common causes: less than ~10GB free on C:, or a broken
    echo network link to the Microsoft CDN. Just re-run this script
    echo to resume where it left off.
    pause
    exit /b %RC%
)

set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
set "VSPATH="
for /f "usebackq tokens=*" %%i in (`"%VSWHERE%" -latest -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VSPATH=%%i"
if defined VSPATH (
    echo.
    echo SUCCESS: C++ Build Tools installed at:
    echo   %VSPATH%
    echo Next step:  scripts\build-windows.bat check
) else (
    echo.
    echo WARNING: vswhere cannot find the C++ toolset yet.
    echo Re-run this script to resume, or inspect the log.
)
pause
endlocal

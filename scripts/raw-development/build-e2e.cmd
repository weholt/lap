@echo off
rem Builds (and optionally runs) the A1-A7 e2e qualification harness
rem (lap-70c / TASK-306) with the correct MSVC + rustup environment.
rem
rem Why this wrapper exists on this machine:
rem   1. The machine PATH carries a standalone Rust stable GNU 1.85 install
rem      ahead of rustup; %USERPROFILE%\.cargo\bin must be prepended so
rem      cargo/rustc resolve to the rustup 1.98 MSVC toolchain (the same
rem      hazard documented in the RapidRAW engine AGENTS.md).
rem   2. In plain (non-developer) shells the MSVC CRT library environment
rem      (LIB/INCLUDE from vcvars64) is not configured, which makes the final
rem      link of the harness fail with LNK1120 (unresolved CRT symbols such
rem      as memcpy/atexit even though /defaultlib:msvcrt is passed).
rem      vcvars64.exe sets LIB/INCLUDE and VS's cmake becomes reachable for
rem      the Lap build script's libjpeg-turbo configure step.
rem
rem Usage:
rem   scripts\raw-development\build-e2e.cmd            (build only)
rem   scripts\raw-development\build-e2e.cmd run <args> (build, then run the
rem       harness binary with <args> forwarded verbatim)
rem
rem Exit code is the build/run exit code.

setlocal enabledelayedexpansion

set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
if not exist "%VSWHERE%" (
    echo build-e2e: vswhere.exe not found; Visual Studio 2022 is required. 1>&2
    exit /b 2
)
for /f "usebackq tokens=*" %%i in (`"%VSWHERE%" -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath`) do set "VSROOT=%%i"
if not defined VSROOT (
    echo build-e2e: no Visual Studio installation with C++ tools found. 1>&2
    exit /b 2
)

set "VCVARS=%VSROOT%\VC\Auxiliary\Build\vcvars64.bat"
if not exist "%VCVARS%" (
    echo build-e2e: vcvars64.bat not found under %VSROOT%. 1>&2
    exit /b 2
)

rem Locate the repository root relative to this script (..\.. from
rem scripts\raw-development\).
set "REPO=%~dp0..\.."

call "%VCVARS%" >nul 2>&1
if not defined LIB (
    echo build-e2e: vcvars64 did not configure LIB; MSVC environment is broken. 1>&2
    exit /b 2
)

set "PATH=%USERPROFILE%\.cargo\bin;%PATH%"

rem The Lap build script also needs cmake (VS-bundled) on PATH for the
rem libjpeg-turbo submodule configure step.
for /f "usebackq delims=" %%i in (`where /r "%VSROOT%" cmake.exe 2^>nul ^| findstr /i "CommonExtensions"`) do set "CMAKE_DIR=%%~dpi"
if defined CMAKE_DIR set "PATH=%CMAKE_DIR%;%PATH%"

where cargo >nul 2>&1 || (
    echo build-e2e: cargo not found after rustup PATH prepend. 1>&2
    exit /b 2
)

rem In non-developer shells rustc's auto-detected MSVC CRT /LIBPATH entries
rem are missing from the final link (LNK1120: unresolved CRT symbols such as
rem memcpy/atexit). Force the VC + Windows SDK library paths explicitly so
rem /defaultlib:msvcrt and the UCRT resolve regardless of shell provenance.
set "MSVC_LIB_DIR=%VSROOT%\VC\Tools\MSVC"
for /f "delims=" %%d in ('dir /b /ad "%MSVC_LIB_DIR%" ^| sort /r') do (
    if exist "!MSVC_LIB_DIR!\%%d\lib\x64\msvcrt.lib" set "MSVC_VERSION_DIR=%%d"
)
if not defined MSVC_VERSION_DIR (
    echo build-e2e: no MSVC lib\x64 directory found under !MSVC_LIB_DIR! 1>&2
    exit /b 2
)
set "SDK_LIB_ROOT=!ProgramFiles(x86)!\Windows Kits\10\lib"
for /f "delims=" %%d in ('dir /b /ad "!SDK_LIB_ROOT!" ^| sort /r') do (
    if exist "!SDK_LIB_ROOT!\%%d\um\x64\kernel32.lib" set "SDK_VERSION_DIR=%%d"
)
if not defined SDK_VERSION_DIR (
    echo build-e2e: no Windows SDK lib directory found under !SDK_LIB_ROOT! 1>&2
    exit /b 2
)
rem RUSTFLAGS is whitespace-split by cargo, so convert the (space-containing)
rem library directories to their short 8.3 forms before embedding them.
rem (Only recorded for diagnosis: with the fake msvcrt.lib removed and LIB
rem configured by vcvars64, these extra /LIBPATH entries are unnecessary.)


rem tauri-build's static_vcruntime step writes an (almost) empty fake
rem msvcrt.lib into the Lap build-script OUT_DIR to force static CRT linking
rem for the packaged application. On this link.exe that fake archive triggers
rem LNK4003 "invalid library format; library ignored", which silently removes
rem the dynamic CRT from resolution and breaks EVERY binary whose dependency
rem graph includes the Lap package (LNK1120: unresolved memcpy/atexit/...).
rem
rem Fix: replace any sub-1KB fake msvcrt.lib with a COPY OF THE REAL
rem msvcrt.lib from the VC toolchain. The build script recreates the fake via
rem create_new only when the file is missing, so a real-content copy persists
rem across build-script reruns. The /NODEFAULTLIB list the script also emits
rem excludes the static CRT names, so the dynamic CRT resolves exactly like a
rem plain (non-tauri) Rust binary on this machine.
set "REAL_MSVCRT="
for /f "delims=" %%d in ('dir /b /ad "!MSVC_LIB_DIR!" ^| sort /r') do (
    if not defined REAL_MSVCRT if exist "!MSVC_LIB_DIR!\%%d\lib\x64\msvcrt.lib" set "REAL_MSVCRT=!MSVC_LIB_DIR!\%%d\lib\x64\msvcrt.lib"
)
if not defined REAL_MSVCRT (
    echo build-e2e: real msvcrt.lib not found under !MSVC_LIB_DIR! 1>&2
    exit /b 2
)
for %%p in (debug release) do (
    for /d %%d in ("%REPO%\tests\raw-development\e2e\target\%%p\build\Lap-*") do (
        if exist "%%d\out\msvcrt.lib" (
            for %%S in ("%%d\out\msvcrt.lib") do (
                if %%~zS LSS 4096 (
                    copy /y "!REAL_MSVCRT!" "%%S" >nul
                    echo build-e2e: replaced fake CRT stub %%S with real msvcrt.lib
                )
            )
        )
    )
)

pushd "%REPO%\tests\raw-development\e2e"
cargo build %*
set "BUILD_CODE=!ERRORLEVEL!"
popd
if not "%BUILD_CODE%"=="0" exit /b %BUILD_CODE%

if /i "%~1"=="run" (
    shift
    "%REPO%\tests\raw-development\e2e\target\debug\lap-raw-e2e.exe" %2 %3 %4 %5 %6 %7 %8 %9
    exit /b !ERRORLEVEL!
)
exit /b 0

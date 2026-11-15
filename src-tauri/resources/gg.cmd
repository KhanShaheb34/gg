@echo off
rem gg launcher, installed by gg @GG_VERSION@
rem
rem gg [path]: opens the git repository that holds path (default: the current folder) in
rem the gg app, and returns at once. Written by the app's "Install 'gg' Command"; the app
rem replaces or removes it only while the marker line above is there. The app is the
rem program it was installed from (below); if it moves, install the command again.
rem (Installing it on Windows comes with the Windows release.)
rem
rem Paths may hold & ( ) ! and spaces. So every value is set with delayed expansion off
rem (a ! in it stays as it is), always inside quotes, and every later use is !VAR! with
rem delayed expansion on, quoted wherever it is an argument; no ( ) block holds a value.
rem No command's output is captured: capturing runs the command through another cmd.
rem Whoever fills in @GG_APP@ must double every % in the path: the line below is parsed
rem with % expansion on.
setlocal DisableDelayedExpansion
set "GG_APP=@GG_APP@"

if "%~1"=="-h" goto help
if "%~1"=="--help" goto help
if "%~1"=="-V" goto version
if "%~1"=="--version" goto version

set "TARGET=%~1"
if "%~1"=="--" set "TARGET=%~2"
if not defined TARGET set "TARGET=."
for %%I in ("%TARGET%") do set "TARGET=%%~fI"
if not exist "%TARGET%\" goto nofolder

where git >nul 2>nul || goto nogit
setlocal EnableDelayedExpansion
rem A drive or share root (C:\) ends with \, which would escape the closing quote of the
rem argument: the trailing \ is taken off, keeping the root (C:\.).
if "!TARGET:~-1!"=="\" set "TARGET=!TARGET!."
git -C "!TARGET!" rev-parse --show-toplevel >nul 2>nul || goto notrepo
rem The app finds the repository's top level itself.
rem Tests put a recording stub here.
if defined GG_APP_BIN goto stub
if not exist "!GG_APP!" goto moved
start "" "!GG_APP!" "!TARGET!"
exit /b 0

:stub
start "" "!GG_APP_BIN!" "!TARGET!"
exit /b 0

:moved
echo gg: the gg app is no longer at !GG_APP!. 1>&2
echo gg: open gg and choose Install 'gg' Command again to point this command at it. 1>&2
exit /b 1

:nofolder
setlocal EnableDelayedExpansion
echo gg: !TARGET!: no such folder 1>&2
exit /b 1

:nogit
echo gg: git was not found on PATH 1>&2
exit /b 1

:notrepo
setlocal EnableDelayedExpansion
echo gg: !TARGET! is not inside a git repository 1>&2
exit /b 1

:version
echo gg @GG_VERSION@
exit /b 0

:help
echo usage: gg [path]
echo        gg -- ^<path starting with -^>
echo.
echo Opens the git repository that holds path (default: the current folder) in gg.
exit /b 0

@echo off
setlocal EnableExtensions DisableDelayedExpansion

if not defined VICTORY_GAME_DIR set "VICTORY_GAME_DIR=%~dp0..\GTA6.Xbox.v1.0.2.1"
if not exist "%VICTORY_GAME_DIR%\game_scarlett_master.exe" (
  echo Set VICTORY_GAME_DIR to your own dump folder containing game_scarlett_master.exe. 1>&2
  echo Example: set "VICTORY_GAME_DIR=D:\Games\MyDump" ^&^& Warmup-Shaders.cmd 1>&2
  exit /b 2
)

set "runner=%~dp0target\release\victory.exe"
if not exist "%runner%" set "runner=%~dp0victory.exe"
if not exist "%runner%" (
  echo Build first with: cargo build --release --locked -p victory 1>&2
  exit /b 2
)

if not defined VICTORY_CACHE_DIR set "VICTORY_CACHE_DIR=%LOCALAPPDATA%\Anaconda\cache"

rem Match the exact translator and pipeline settings used by Play-Anaconda.cmd / Play-Anaconda-HighEnd.cmd
rem so every precompiled shader and driver PSO hits the cache on launch.
if not defined VICTORY_SHADER_OPT set "VICTORY_SHADER_OPT=63"
if not defined VICTORY_EARLY_DEPTH set "VICTORY_EARLY_DEPTH=1"
if not defined VICTORY_THREAD_TICKET set "VICTORY_THREAD_TICKET=1"
if not defined VICTORY_RAW_VIEW_CLAMP set "VICTORY_RAW_VIEW_CLAMP=1"
if not defined VICTORY_WATER_TILE_SLOTS set "VICTORY_WATER_TILE_SLOTS=1"
if not defined VICTORY_SAMPLER_ORDER set "VICTORY_SAMPLER_ORDER=1"
if not defined VICTORY_PIPELINE_BOOK set "VICTORY_PIPELINE_BOOK=1"
if not defined VICTORY_RT_NO_ARENAS set "VICTORY_RT_NO_ARENAS=1"
if not defined VICTORY_LIBRARY_LOCK set "VICTORY_LIBRARY_LOCK=1"
if not defined VICTORY_LIBRARY_MAP set "VICTORY_LIBRARY_MAP=0"

echo [1/2] Pre-translating all GTA VI shaders into %VICTORY_CACHE_DIR%\shaders ...
"%runner%" shaders "%VICTORY_GAME_DIR%"
if errorlevel 1 exit /b %errorlevel%

echo.
if exist "%VICTORY_CACHE_DIR%\pipelines\known-pipelines.bin" (
  echo [2/2] Warming GPU driver pipeline cache from known-pipelines.bin in deterministic static-sampler order ...
  "%runner%" warm
  if errorlevel 1 exit /b %errorlevel%
) else (
  echo [2/2] No known-pipelines.bin recorded yet. Launch the game once with VICTORY_PIPELINE_BOOK=1, then re-run Warmup-Shaders.cmd to pre-warm driver PSOs.
)

echo.
echo Shader warmup complete!
exit /b 0

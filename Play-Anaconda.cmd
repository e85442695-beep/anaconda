@echo off
setlocal EnableExtensions DisableDelayedExpansion

if not defined VICTORY_GAME_DIR set "VICTORY_GAME_DIR=%~dp0..\GTA6.Xbox.v1.0.2.1"
if not exist "%VICTORY_GAME_DIR%\game_scarlett_master.exe" (
  echo Set VICTORY_GAME_DIR to your own dump folder containing game_scarlett_master.exe. 1>&2
  echo Example: set "VICTORY_GAME_DIR=D:\Games\MyDump" ^&^& Play-Anaconda.cmd 1>&2
  exit /b 2
)

set "runner=%~dp0target\release\victory.exe"
if not exist "%runner%" set "runner=%~dp0victory.exe"
if not exist "%runner%" (
  echo Build first with: cargo build --release --locked -p victory 1>&2
  exit /b 2
)

if not defined VICTORY_CACHE_DIR set "VICTORY_CACHE_DIR=%LOCALAPPDATA%\Anaconda\cache"
if not defined VICTORY_DATA_DIR set "VICTORY_DATA_DIR=%LOCALAPPDATA%\Anaconda\data-s"

rem 1. <= 4 GB VRAM budget + tiled top-mip drop + anti-black packed tail fallback
if not defined VICTORY_VRAM_MB set "VICTORY_VRAM_MB=3200"
if not defined VICTORY_TEXTURE_ROOM_PERCENT set "VICTORY_TEXTURE_ROOM_PERCENT=92"
if not defined VICTORY_LEVELS_ON_DEMAND set "VICTORY_LEVELS_ON_DEMAND=1"
if not defined VICTORY_LEVELS_MIN_KB set "VICTORY_LEVELS_MIN_KB=128"
if not defined VICTORY_TILES_DROP set "VICTORY_TILES_DROP=2"
if not defined VICTORY_TILES_DROP_MIN set "VICTORY_TILES_DROP_MIN=512"
if not defined VICTORY_TILES_TAIL_ALWAYS set "VICTORY_TILES_TAIL_ALWAYS=1"
if not defined VICTORY_TEXTURE_DROP set "VICTORY_TEXTURE_DROP=0"
if not defined VICTORY_CARD_ROOM_FOR_TARGETS_MB set "VICTORY_CARD_ROOM_FOR_TARGETS_MB=768"
if not defined VICTORY_CARD_BUFFERS_MIN_KB set "VICTORY_CARD_BUFFERS_MIN_KB=64"
if not defined VICTORY_CARD_BUFFERS_MB set "VICTORY_CARD_BUFFERS_MB=512"
if not defined VICTORY_CARD_WINDOWS_MB set "VICTORY_CARD_WINDOWS_MB=0"

rem 2. Shader optimizations + safe NVIDIA pipeline library
if not defined VICTORY_SHADER_OPT set "VICTORY_SHADER_OPT=63"
if not defined VICTORY_LIBRARY_LOCK set "VICTORY_LIBRARY_LOCK=1"
if not defined VICTORY_LIBRARY_MAP set "VICTORY_LIBRARY_MAP=0"
if not defined VICTORY_VSYNC set "VICTORY_VSYNC=1"

rem 3. No hardware Ray Tracing: 16x faster half-grid ambient GI + indoor floor
if not defined VICTORY_AMBIENT set "VICTORY_AMBIENT=1.0"
if not defined VICTORY_AMBIENT_FAST set "VICTORY_AMBIENT_FAST=1"
if not defined VICTORY_AMBIENT_FLOOR set "VICTORY_AMBIENT_FLOOR=0.04"
if not defined VICTORY_AMBIENT_AIR set "VICTORY_AMBIENT_AIR=1.5"

rem 4. Texture streaming & CPU command-list pipeline
if not defined VICTORY_THIN set "VICTORY_THIN=1"
if not defined VICTORY_JOIN_PARTS set "VICTORY_JOIN_PARTS=8"
if not defined VICTORY_TEXTURE_NO_WAIT set "VICTORY_TEXTURE_NO_WAIT=1"
if not defined VICTORY_UPLOAD_AHEAD set "VICTORY_UPLOAD_AHEAD=1"
if not defined VICTORY_RELAY set "VICTORY_RELAY=1"
if not defined VICTORY_RELAY_COPY set "VICTORY_RELAY_COPY=1"
if not defined VICTORY_WIDE_BACK set "VICTORY_WIDE_BACK=1"
if not defined VICTORY_CARD_REHASH_RUNS set "VICTORY_CARD_REHASH_RUNS=1"
if not defined VICTORY_JOIN_COPIES set "VICTORY_JOIN_COPIES=1"
if not defined VICTORY_INDIRECT_WINDOW set "VICTORY_INDIRECT_WINDOW=1"
if not defined VICTORY_CARD_STEPS set "VICTORY_CARD_STEPS=2"
if not defined VICTORY_INDIRECT_BATCH set "VICTORY_INDIRECT_BATCH=2"
if not defined VICTORY_MAP_THREADS set "VICTORY_MAP_THREADS=1"
if not defined VICTORY_CARD_AWAKE set "VICTORY_CARD_AWAKE=1"
if not defined VICTORY_SHOW_AFTER set "VICTORY_SHOW_AFTER=1"

rem 5. Built-in Series S FSR + NaN-safe RCAS GPU compute upscaler (720p target)
if not defined VICTORY_FSR_SCALE set "VICTORY_FSR_SCALE=1.7"
if not defined VICTORY_FSR_SHARPEN set "VICTORY_FSR_SHARPEN=0.50"

if not defined VICTORY_LOG if not defined RUST_LOG set "VICTORY_LOG=warn"

"%runner%" run "%VICTORY_GAME_DIR%\game_scarlett_master.exe" --console series-s --execute
exit /b %errorlevel%

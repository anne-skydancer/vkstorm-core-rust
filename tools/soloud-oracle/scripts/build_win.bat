@echo off
rem Builds the SoLoud oracle and the asset-decode oracle with MSVC (VS 2022 x64).
rem   build_win.bat <viewer> <outdir>
rem <viewer> is the viewer checkout with a configured build-vc170-64 (its
rem autobuild packages provide the 3p-soloud and ogg_vorbis libraries).
setlocal
set V=%~1
set OUT=%~2
if "%V%"=="" set V=C:\Dev\vkstorm-rust-audio
if "%OUT%"=="" set OUT=%~dp0..\bin
set P=%V%\build-vc170-64\packages
set C=%~dp0..\c
if not exist "%OUT%" mkdir "%OUT%"
call "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" >nul || exit /b 1
cd /d "%OUT%" || exit /b 1
rem The adapter code is compiled like the viewer's llaudio (/O2 /arch:AVX2
rem /fp:precise); SoLoud itself is the package's prebuilt soloud.lib.
cl /nologo /O2 /arch:AVX2 /fp:precise /EHsc /MD /std:c++17 /DNDEBUG /D_CRT_SECURE_NO_WARNINGS ^
  /I"%C%\shim" /I"%V%\indra\llaudio" /I"%P%\include\soloud" ^
  "%C%\soloud_oracle.cpp" /Fe:soloud_oracle.exe ^
  /link /LIBPATH:"%P%\lib\release" soloud.lib ole32.lib user32.lib advapi32.lib winmm.lib || exit /b 1
cl /nologo /O2 /arch:AVX2 /fp:precise /MD /DNDEBUG /D_CRT_SECURE_NO_WARNINGS ^
  /I"%P%\include" "%C%\asset_oracle.c" /Fe:asset_oracle.exe ^
  /link /LIBPATH:"%P%\lib\release" libvorbisfile.lib libvorbis.lib libogg.lib || exit /b 1

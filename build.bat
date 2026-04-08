@echo off
set MSBuildPath=
for %%p in (
	"%ProgramFiles%\Microsoft Visual Studio\2022\Community\MSBuild\Current\Bin\"
	"%ProgramFiles%\Microsoft Visual Studio\2022\Professional\MSBuild\Current\Bin\"
	"%ProgramFiles%\Microsoft Visual Studio\2022\Enterprise\MSBuild\Current\Bin\"
	"%ProgramFiles(x86)%\Microsoft Visual Studio\2022\BuildTools\MSBuild\Current\Bin\"
	"%ProgramFiles(x86)%\Microsoft Visual Studio\2019\Community\MSBuild\Current\Bin\"
	"%ProgramFiles(x86)%\Microsoft Visual Studio\2019\Professional\MSBuild\Current\Bin\"
	"%ProgramFiles(x86)%\Microsoft Visual Studio\2019\Enterprise\MSBuild\Current\Bin\"
) do (
	if exist %%p (
		set "MSBuildPath=%%~p"
		goto :found
	)
)
echo Please download Microsoft Visual Studio Build Tools
echo https://visualstudio.microsoft.com/downloads/#build-tools-for-visual-studio-2022
pause
exit /b 1

:found
echo Using MSBuild from: %MSBuildPath%
"%MSBuildPath%MSBuild.exe" MagicRemoteService.sln /p:Configuration=Debug /p:Platform="Any CPU" /clp:Summary /nologo
"%MSBuildPath%MSBuild.exe" MagicRemoteService.sln /p:Configuration=Release /p:Platform="Any CPU" /clp:Summary /nologo
pause

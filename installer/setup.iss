; Inno Setup скрипт для ScreenSnip.
; Собрать: открыть в Inno Setup Compiler (ISCC.exe setup.iss) ПОСЛЕ `cargo build --release`.
; Автозапуск настраивает само приложение (галка в конфиге), здесь его НЕ трогаем.

#define MyAppName "ScreenSnip"
#define MyAppVersion "0.1.0"
#define MyAppExeName "screensnip.exe"
#define MyAppPublisher "ScreenSnip"

[Setup]
; AppId должен оставаться НЕИЗМЕННЫМ между версиями — по нему Inno распознаёт
; уже установленную программу и обновляет её на месте.
AppId={{B7E2F3A1-9C4D-4E6B-8A2F-1D3C5E7A9B0C}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
; Имя мьютекса должно совпадать с MUTEX_NAME в src/single_instance.rs —
; так установщик увидит запущенную копию и предложит её закрыть перед обновлением/удалением.
AppMutex=ScreenSnip_SingleInstance
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
; Установка в Program Files требует прав администратора.
PrivilegesRequired=admin
OutputDir=output
OutputBaseFilename=ScreenSnip-Setup-{#MyAppVersion}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesInstallIn64BitMode=x64compatible

[Languages]
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Tasks]
Name: "launchafterinstall"; Description: "Запустить {#MyAppName} после установки"; GroupDescription: "Дополнительно:"

[Files]
; Путь относительно installer\: ..\target\release\screensnip.exe
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"
Name: "{group}\Удалить {#MyAppName}"; Filename: "{uninstallexe}"

[Run]
; Запуск после установки (без ожидания), если выбрана задача.
Filename: "{app}\{#MyAppExeName}"; Description: "Запустить {#MyAppName}"; Flags: nowait postinstall skipifsilent; Tasks: launchafterinstall

[UninstallRun]
; При удалении завершаем процесс, чтобы не держал файл.
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM {#MyAppExeName}"; Flags: runhidden; RunOnceId: "KillScreenSnip"
; Удаляем запись автозапуска (её создаёт само приложение в HKCU\...\Run),
; иначе после удаления останется «осиротевшая» ссылка на несуществующий exe.
Filename: "{sys}\reg.exe"; Parameters: "delete ""HKCU\Software\Microsoft\Windows\CurrentVersion\Run"" /v ScreenSnip /f"; Flags: runhidden; RunOnceId: "DelAutostart"

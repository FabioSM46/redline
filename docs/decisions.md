# Decisioni

Ogni decisione ha un numero (D1, D2, …) che il codice e le issue possono citare. Una decisione
cambiata si aggiorna qui, con la data e il motivo.

## D1 — Stack: Rust + Tauri 2, frontend React + TypeScript (2026-10-29)

Lo stesso stack di wow-don: backend Rust con Tauri 2, frontend React + TypeScript con Vite e
pnpm. La pipeline di sviluppo (skill, CI, review) è portata da wow-don.

## D2 — Coordinare strumenti esistenti, non riscriverli (2026-10-29)

- **Sensori**: lettura diretta da sysfs (`/sys/class/drm/cardN/device` e il suo `hwmon`), senza
  permessi.
- **Tuning**: demone LACT (`lactd`), tramite il suo socket Unix (`/run/lactd.sock`, una
  richiesta JSON per riga). LACT è MIT e gira come root al posto nostro.
- **Replay e registrazione**: GPU Screen Recorder come processo esterno, pilotato con opzioni,
  segnali e IPC. È GPL-3.0: niente codice copiato.
- **Overlay**: MangoHud, configurato da Redline.

Riscrivere overlay Vulkan, cattura video o un demone privilegiato costerebbe anni e produrrebbe
copie peggiori di progetti maturi.

## D3 — Solo Linux (2026-10-29)

Redline legge il driver `amdgpu` del kernel e usa strumenti solo Linux. Su Windows esiste già
Adrenaline. La CI e i pacchetti sono solo Linux.

## D4 — Distribuzione: `.deb`, configurazione guidata, poi un repository apt (2026-10-29)

Il pacchetto principale è un `.deb` generato da Tauri. LACT e GPU Screen Recorder non sono nei
repository di Ubuntu, quindi il `.deb` non può installarli tramite `Depends`.

1. **Subito**: all'avvio l'app controlla le dipendenze e propone di installare quelle mancanti,
   chiedendo i permessi con `pkexec`. Le funzioni senza dipendenza restano disattivate.
2. **Più avanti**: un repository apt con `redline`, `lact` e `gpu-screen-recorder`, così
   `apt install redline` installa tutto e gli aggiornamenti arrivano con `apt upgrade`.

Scartati:
- script di installazione del `.deb` che scaricano pacchetti: si bloccano senza rete e
  confondono apt;
- binari inclusi nel nostro `.deb`: per GPU Screen Recorder servono gli obblighi GPL e FFmpeg 8,
  per LACT c'è il rischio di conflitto con un'installazione già presente;
- Redline come Flatpak: la sandbox ostacola l'accesso a sysfs e al socket di `lactd`.

## D5 — Scorciatoie globali (2026-10-29)

Sotto Wayland le scorciatoie globali passano dal portale GlobalShortcuts. GNOME lo implementa
dalla 48; Ubuntu 24.04 ha GNOME 46. Finché il portale non c'è, Redline offrirà dei comandi
(per esempio "salva replay") da legare alle scorciatoie personalizzate del desktop.

## D6 — Licenza MIT (2026-10-29)

Redline è MIT. GPU Screen Recorder (GPL-3.0) resta un processo separato, quindi non vincola la
licenza di Redline.

## Aperte

- Come arriva l'overlay: configurazione di MangoHud gestita da Redline o un overlay proprio.
- Dove vive il replay quando l'app è chiusa: servizio utente systemd o processo dell'app.

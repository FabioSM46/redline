# Architettura

```
┌──────────────────────── Redline (utente, mai root) ────────────────────────┐
│  Frontend React (src/)  ──invoke()──▶  App Tauri (src-tauri/src/)          │
│                                              │                             │
│                                       redline-core                         │
│                    ┌───────────────┬─────────┴──────────┬────────────────┐ │
│                    │ sensors       │ lact               │ recorder       │ │
└────────────────────┼───────────────┼────────────────────┼────────────────┼─┘
                     ▼               ▼                    ▼                ▼
               sysfs amdgpu   /run/lactd.sock     gpu-screen-recorder   MangoHud
               (sola lettura) (lactd, root)       (processo esterno)    (config)
```

## Moduli di `redline-core`

| Modulo | Responsabilità | Stato |
|---|---|---|
| `sensors` | Trova le GPU AMD in `/sys/class/drm`, legge temperature, potenza, ventola, clock, tensione, utilizzo e VRAM | prototipo, testato su RDNA4 |
| `lact` | Client del socket di `lactd`: dispositivi, statistiche, limite di potenza, conferma o ritorno delle modifiche | prototipo |
| `recorder` | Avvia GPU Screen Recorder in modalità replay, salva il replay, avvia e ferma la registrazione, una traccia audio per sorgente | prototipo |

## Flussi

**Statistiche**: il frontend chiama `gpu_snapshot` ogni secondo; il backend legge sysfs. Nessun
permesso, nessun demone.

**Tuning**: il frontend chiede una modifica → il backend la manda a `lactd` → `lactd` la applica
in attesa → l'utente conferma o annulla → il backend manda `confirm_pending_config`. Senza
conferma `lactd` torna da solo ai valori precedenti.

**Replay**: il backend avvia GPU Screen Recorder con il buffer in RAM e una traccia audio per
ogni sorgente scelta (`-a` ripetuto). Il salvataggio è un segnale (o un comando IPC); GPU Screen
Recorder scrive su stdout il percorso del file salvato e il backend lo inoltra alla UI.

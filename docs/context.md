# Contesto

## Perché esiste Redline

Chi passa da Windows a Linux per giocare con una GPU AMD perde **AMD Software: Adrenaline
Edition**: overlay con FPS e temperature, grafici delle prestazioni, tuning di ventole e potenza,
replay istantanei dell'ultimo minuto e registrazione con una scorciatoia. AMD non lo porta su
Linux.

Su Linux ogni pezzo esiste già come progetto separato:

| Funzione | Progetto |
|---|---|
| Overlay in gioco | MangoHud |
| Tuning (ventole, potenza, clock, undervolt) | LACT, CoreCtrl |
| Replay e registrazione con encoder hardware | GPU Screen Recorder, OBS |
| Monitoraggio | amdgpu_top, Mission Center |

Il problema è che sono cinque programmi, con cinque interfacce e cinque configurazioni. Redline
è **un'unica app** che li coordina, con un'interfaccia che chi viene da Adrenaline riconosce.

## Cosa prende da Adrenaline e cosa lascia

Prende: dashboard con le metriche principali, grafici nel tempo, overlay, tuning con profili,
replay istantaneo, registrazione, scorciatoie.

Lascia, perché la usa una minoranza minima: browser e social integrati, streaming integrato,
statistiche di gioco online, avvisi su driver e giochi, impostazioni grafiche per gioco che su
Linux appartengono a Mesa e Proton, impostazioni dello schermo che appartengono al desktop.

## Per chi è

Giocatori con GPU AMD su una distribuzione Linux recente, sotto Wayland o X11. Il riferimento è
Ubuntu LTS con GNOME, ma Redline non dipende da una distribuzione o da un desktop.

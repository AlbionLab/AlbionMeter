// The React side of the dungeon journal pairs: web/src/main.tsx does not mount DungeonJournal or HistoryTransfer, so
// this page mounts them, unchanged, inside the same top bar (main.tsx's markup with a fourth "Dungeons" view, as the
// Slint app shows it behind --journal). Built by rust/scripts/ui-parity.mjs.
import { createRoot } from 'react-dom/client';
import { useState } from 'react';
import { DungeonJournal } from '../../web/src/Library';
import { HistoryTransfer } from '../../web/src/HistoryTransfer';
import { WindowControls } from '../../web/src/DesktopChrome';
import { Icon } from '../../web/src/Icon';
import { loadLanguage } from '../../web/src/i18n';
import '../../web/src/styles.css';

function Journal() {
  const [path, setPath] = useState('/dungeons');
  const match = /^\/dungeons(?:\/([^/]+))?(?:\/floor\/(\d+))?/.exec(path);
  return <div className="mini-shell desktop">
    <header className="mini-bar">
      <span className="brand"><img src="/mark.svg" alt="" /><span className="brand-name">Albion Meter</span><span className="brand-version">v0.2.0</span></span>
      <nav className="segmented">
        <button><Icon name="activity" size={14} /><span className="nav-label">Meter</span></button>
        <button><Icon name="database" size={14} /><span className="nav-label">Capture</span></button>
        <button><Icon name="sword" size={14} /><span className="nav-label">Game data</span></button>
        <button className="active" aria-pressed="true"><Icon name="layers" size={14} /><span className="nav-label">Dungeons</span></button>
      </nav>
      <div className="segmented language-switch"><button className="active">EN</button><button>PT</button></div>
      <span className="capture-pill live" role="status"><span className="status-dot" /><span className="pill-label">Recording</span></span>
      <WindowControls />
    </header>
    <main className="mini-main">
      <HistoryTransfer onImported={() => {}} />
      <DungeonJournal base="/dungeons" runId={match?.[1]} floor={Number(match?.[2] ?? 0)} navigate={setPath} />
    </main>
  </div>;
}

void loadLanguage().then(() => createRoot(document.getElementById('root')!).render(<Journal />));

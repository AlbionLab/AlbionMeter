export type UpdateState =
  | { state: 'idle' | 'checking' | 'current' }
  | { state: 'downloading'; version: string; percent: number; transferred?: number; total?: number }
  | { state: 'ready' | 'installing'; version: string }
  | { state: 'error'; message: string };

export interface DesktopInfo { version: string; platform: string; maximized: boolean; update: UpdateState }

export interface Desktop {
  info: () => Promise<DesktopInfo>;
  window: (action: 'minimize' | 'maximize' | 'close') => void;
  onWindowState: (callback: (state: { maximized: boolean }) => void) => () => void;
  onUpdate: (callback: (state: UpdateState) => void) => () => void;
  update: (action: 'install' | 'check') => void;
}

export const desktop = (window as unknown as { albionDesktop?: Desktop }).albionDesktop;

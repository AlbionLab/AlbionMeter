const { contextBridge, ipcRenderer } = require('electron');

const listen = (channel, callback) => {
  const handler = (_event, value) => callback(value);
  ipcRenderer.on(channel, handler);
  return () => ipcRenderer.removeListener(channel, handler);
};

contextBridge.exposeInMainWorld('albionDesktop', {
  info: () => ipcRenderer.invoke('desktop:info'),
  window: action => ipcRenderer.send('desktop:window', action),
  onWindowState: callback => listen('desktop:window-state', callback),
  onUpdate: callback => listen('desktop:update', callback),
  update: action => ipcRenderer.send('desktop:update', action),
  onStartup: callback => listen('desktop:startup', callback),
  startup: action => ipcRenderer.send('desktop:startup', action),
  permission: () => ipcRenderer.invoke('desktop:permission'),
});

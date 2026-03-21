import { app, BrowserWindow, Menu, type MenuItemConstructorOptions } from 'electron'
import { createWindow } from './window-manager'

export function createMenu(): void {
  const isMac = process.platform === 'darwin'

  const template: MenuItemConstructorOptions[] = [
    // App menu (macOS only)
    ...(isMac
      ? [
          {
            label: app.name,
            submenu: [
              { role: 'about' as const },
              { type: 'separator' as const },
              { role: 'services' as const },
              { type: 'separator' as const },
              { role: 'hide' as const },
              { role: 'hideOthers' as const },
              { role: 'unhide' as const },
              { type: 'separator' as const },
              { role: 'quit' as const }
            ]
          }
        ]
      : []),

    // Edit
    {
      label: 'Edit',
      submenu: [
        { role: 'undo' },
        { role: 'redo' },
        { type: 'separator' },
        { role: 'cut' },
        { role: 'copy' },
        { role: 'paste' },
        { role: 'selectAll' }
      ]
    },

    // View
    {
      label: 'View',
      submenu: [
        {
          label: 'New Window',
          accelerator: 'CmdOrCtrl+Shift+N',
          click: (): void => {
            createWindow()
          }
        },
        { type: 'separator' },
        { role: 'zoomIn' },
        { role: 'zoomOut' },
        { role: 'resetZoom' },
        { type: 'separator' },
        {
          label: 'Terminal Zoom In',
          accelerator: 'CmdOrCtrl+Shift+=',
          click: (): void => {
            const win = BrowserWindow.getFocusedWindow()
            if (win) win.webContents.send('terminal:zoom-in')
          }
        },
        {
          label: 'Terminal Zoom Out',
          accelerator: 'CmdOrCtrl+Shift+-',
          click: (): void => {
            const win = BrowserWindow.getFocusedWindow()
            if (win) win.webContents.send('terminal:zoom-out')
          }
        },
        {
          label: 'Terminal Reset Zoom',
          accelerator: 'CmdOrCtrl+Shift+0',
          click: (): void => {
            const win = BrowserWindow.getFocusedWindow()
            if (win) win.webContents.send('terminal:zoom-reset')
          }
        },
        { type: 'separator' },
        { role: 'toggleDevTools' },
        { type: 'separator' },
        { role: 'togglefullscreen' },
        { type: 'separator' },
        {
          label: 'Settings',
          accelerator: 'CmdOrCtrl+,',
          click: (): void => {
            const win = BrowserWindow.getFocusedWindow()
            if (win) win.webContents.send('menu:open-settings')
          }
        }
      ]
    },

    // Window
    {
      label: 'Window',
      submenu: [
        { role: 'minimize' },
        { role: 'zoom' },
        ...(isMac
          ? [
              { type: 'separator' as const },
              { role: 'front' as const }
            ]
          : []),
        { role: 'close' }
      ]
    }
  ]

  const menu = Menu.buildFromTemplate(template)
  Menu.setApplicationMenu(menu)
}

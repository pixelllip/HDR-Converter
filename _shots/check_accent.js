const { app, systemPreferences, nativeTheme } = require('electron')
app.commandLine.appendSwitch('no-sandbox')
app.whenReady().then(() => {
  console.log('getAccentColor:', systemPreferences.getAccentColor())
  console.log('shouldUseDarkColors:', nativeTheme.shouldUseDarkColors)
  console.log('getColor windowsAccent:', (() => { try { return systemPreferences.getColor('windows-accent') } catch (e) { return 'n/a: ' + e.message } })())
  app.exit(0)
})

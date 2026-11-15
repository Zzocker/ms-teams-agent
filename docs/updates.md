# Updating Teams

[Back to README](../README.md)

The update icon below the theme button (**System**, **Dark**, or **Light**) in the left navigation opens the updates panel and checks for a
newer version. Before sign-in, the same icon appears in the top-left corner. The panel shows your
installed version, progress, the last successful check, and a retry action if the check fails. Close it with **Close**, Escape,
or a click outside; an ongoing check continues in the background.

Checks run only when requested and contact GitHub without your Teams credentials. They include
published preview releases. When an update is available, the update icon shows a notification dot.
**Download update** opens the newer release's page in your browser, and the panel explains how
to install it on your operating system.

Quit Teams using the tray's **Quit Teams** action (macOS: Quit from the application menu),
then download and install the new archive:

- Linux: extract the archive and run `sh install.sh` again.
- Windows: replace the executable in your existing installation folder.
- macOS: replace `/Applications/Teams.app` with the new app.

Reopen Teams. Settings and sign-in data live outside the installation and are preserved.
Closing the window alone leaves Teams running; quit it before updating.
Updates are installed manually; the app does not replace itself.

// Test programs of scripts/beta-test.ps1, one source for all of them.
//
// "beta-test.ps1 setup" compiles this file with the .NET Framework compiler
// that ships with Windows (csc.exe, C# 5: no newer syntax here) and copies the
// result to a few names in the test folder. What a copy does is chosen by
// --role, so the programs are real windows and tray icons that kaltstellen can
// freeze, close and start again, and that have nothing to lose.
//
//   --role beat      window; writes a heartbeat (a number that rises) every
//                    100 ms to --beat FILE, so a freeze shows as a standstill.
//                    Closes when asked to (WM_CLOSE).
//   --role stubborn  like beat, but refuses to close: it stays and kaltstellen
//                    reports it (on_timeout = "keep") or ends it ("kill").
//   --role tray      no window, a notification-area icon only (T22).
//   --alloc-mb N     keeps N MB of memory in use and touches it, for the trim
//                    measurement (T19).
//   --title TEXT     window title (default: the file name without .exe).
//
// Nothing here is shipped with kaltstellen and nothing leaves the machine:
// the program only writes the heartbeat file it is given.

using System;
using System.Drawing;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;
using System.Windows.Forms;

internal static class BetaProgram
{
    private static string role = "beat";
    private static string beat = null;
    private static int allocMb = 0;
    private static string title = null;
    private static IntPtr block = IntPtr.Zero;
    private static long blockBytes = 0;

    [STAThread]
    private static int Main(string[] args)
    {
        for (int i = 0; i < args.Length; i++)
        {
            string a = args[i];
            string next = i + 1 < args.Length ? args[i + 1] : null;
            if (a == "--role" && next != null) { role = next; i++; }
            else if (a == "--beat" && next != null) { beat = next; i++; }
            else if (a == "--title" && next != null) { title = next; i++; }
            else if (a == "--alloc-mb" && next != null)
            {
                int.TryParse(next, out allocMb);
                i++;
            }
        }
        if (title == null)
        {
            title = Path.GetFileNameWithoutExtension(
                System.Diagnostics.Process.GetCurrentProcess().MainModule.FileName);
        }
        if (allocMb > 0) { Allocate(); }

        Thread heart = new Thread(HeartLoop);
        heart.IsBackground = true;
        heart.Start();

        Application.EnableVisualStyles();
        if (role == "tray") { RunTray(); }
        else { Application.Run(new BetaForm(title, role == "stubborn")); }
        return 0;
    }

    // Memory that is really in use: committed and written to, so it counts
    // towards the working set until kaltstellen moves it out (trim).
    private static void Allocate()
    {
        blockBytes = (long)allocMb * 1024 * 1024;
        block = Marshal.AllocHGlobal(new IntPtr(blockBytes));
        for (long off = 0; off < blockBytes; off += 4096)
        {
            Marshal.WriteByte(block, (int)Math.Min(off, int.MaxValue), 1);
        }
    }

    // The heartbeat: DateTime ticks, so every write differs from the last.
    // A frozen process stops writing, which is what the script measures.
    private static void HeartLoop()
    {
        long touch = 0;
        while (true)
        {
            if (beat != null)
            {
                try
                {
                    string temp = beat + ".tmp";
                    File.WriteAllText(temp, DateTime.UtcNow.Ticks.ToString());
                    // Replaced in one step: a reader never finds the file missing.
                    if (File.Exists(beat)) { File.Replace(temp, beat, null); }
                    else { File.Move(temp, beat); }
                }
                catch (IOException) { }
                catch (UnauthorizedAccessException) { }
            }
            if (block != IntPtr.Zero)
            {
                // One page now and then keeps the memory part of the working set.
                touch = (touch + 4096 * 64) % blockBytes;
                Marshal.WriteByte(block, (int)Math.Min(touch, int.MaxValue), 2);
            }
            Thread.Sleep(100);
        }
    }

    private static void RunTray()
    {
        NotifyIcon icon = new NotifyIcon();
        icon.Icon = SystemIcons.Information;
        icon.Text = title;
        icon.Visible = true;
        try { Application.Run(); }
        finally { icon.Visible = false; icon.Dispose(); }
    }
}

internal sealed class BetaForm : Form
{
    private readonly bool stubborn;

    internal BetaForm(string title, bool stubborn)
    {
        this.stubborn = stubborn;
        Text = title;
        ClientSize = new Size(320, 80);
        StartPosition = FormStartPosition.Manual;
        // Away from the middle, where the windows of the test are looked at.
        Location = new Point(40 + ((title.GetHashCode() & 0x7fffffff) % 6) * 30, 40);
        ShowInTaskbar = true;
        Label label = new Label();
        label.Dock = DockStyle.Fill;
        label.TextAlign = ContentAlignment.MiddleCenter;
        label.Text = stubborn
            ? "Test program of the kaltstellen beta test (does not close)"
            : "Test program of the kaltstellen beta test";
        Controls.Add(label);
    }

    // A refusing program answers the close request and stays: what a program
    // with unsaved work does.
    protected override void OnFormClosing(FormClosingEventArgs e)
    {
        if (stubborn && e.CloseReason == CloseReason.UserClosing)
        {
            e.Cancel = true;
            return;
        }
        base.OnFormClosing(e);
    }
}

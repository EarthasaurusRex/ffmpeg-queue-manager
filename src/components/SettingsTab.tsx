import { useState, useEffect } from "react";
import { enable, disable, isEnabled } from "@tauri-apps/plugin-autostart";
import { Switch, SpinButton, Field, Button, tokens } from "@fluentui/react-components";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

interface SettingsTabProps {
  maxConcurrency: number;
  setMaxConcurrency: (val: number) => void;
  autoClear: boolean;
  setAutoClear: (val: boolean) => void;
  classes: any;
}

export default function SettingsTab({ maxConcurrency, setMaxConcurrency, autoClear, setAutoClear, classes }: SettingsTabProps) {
  const [autostartEnabled, setAutostartEnabled] = useState(false);
  const [updateStatus, setUpdateStatus] = useState("Idle");

  const checkForUpdates = async () => {
    try {
      setUpdateStatus("Checking for updates...");
      const update = await check();
      if (update) {
        setUpdateStatus(`Downloading update ${update.version}...`);
        await update.downloadAndInstall((event) => {
          if (event.event === "Progress") {
            // we could track progress here, but simple message is fine
          }
        });
        setUpdateStatus("Update installed. Restarting...");
        await relaunch();
      } else {
        setUpdateStatus("You are on the latest version.");
        setTimeout(() => setUpdateStatus("Idle"), 3000);
      }
    } catch (err) {
      setUpdateStatus(`Error: ${err}`);
    }
  };

  useEffect(() => {
    // Check initial autostart status
    const checkAutostart = async () => {
      try {
        const enabled = await isEnabled();
        setAutostartEnabled(enabled);
      } catch (err) {
        console.error("Failed to check autostart status", err);
      }
    };
    checkAutostart();
  }, []);

  const handleAutostartToggle = async (_ev: any, data: { checked: boolean }) => {
    try {
      if (data.checked) {
        await enable();
      } else {
        await disable();
      }
      setAutostartEnabled(data.checked);
    } catch (err) {
      alert("Failed to toggle Run on Startup: " + err);
    }
  };

  return (
    <div className={classes.profilesGrid}>
      <div className={classes.profileCard}>
        <h3>Application Settings</h3>
        
        <div style={{ marginTop: 20, display: "flex", flexDirection: "column", gap: 15 }}>
          <Field label="Run on Startup" hint="Automatically launch the app silently in the background when you log in.">
            <Switch 
              checked={autostartEnabled} 
              onChange={handleAutostartToggle} 
            />
          </Field>

          <Field label="Max Concurrent Encoding Jobs" hint="How many videos to process at the exact same time.">
            <SpinButton 
              min={1} 
              max={16} 
              value={maxConcurrency} 
              onChange={(_e, data) => {
                if (data.value !== undefined && data.value !== null) setMaxConcurrency(data.value);
              }} 
            />
          </Field>

          <Field label="Auto-clear Completed Jobs" hint="Remove jobs from the queue once they finish successfully.">
            <Switch 
              checked={autoClear} 
              onChange={(_e, data) => setAutoClear(data.checked)} 
            />
          </Field>

          <Field label="Updates" hint="Check for new versions from GitHub and install them.">
            <div style={{ display: "flex", gap: "10px", alignItems: "center", marginTop: "5px" }}>
              <Button onClick={checkForUpdates} disabled={updateStatus !== "Idle" && updateStatus !== "You are on the latest version."}>
                Check for Updates
              </Button>
              {updateStatus !== "Idle" && (
                <span style={{ fontSize: "12px", color: tokens.colorNeutralForeground3 }}>{updateStatus}</span>
              )}
            </div>
          </Field>
        </div>
      </div>
    </div>
  );
}

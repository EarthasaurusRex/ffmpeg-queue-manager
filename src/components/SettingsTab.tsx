import { useState, useEffect } from "react";
import { enable, disable, isEnabled } from "@tauri-apps/plugin-autostart";
import { Switch, SpinButton, Field } from "@fluentui/react-components";

interface SettingsTabProps {
  maxConcurrency: number;
  setMaxConcurrency: (val: number) => void;
  autoClear: boolean;
  setAutoClear: (val: boolean) => void;
  classes: any;
}

export default function SettingsTab({ maxConcurrency, setMaxConcurrency, autoClear, setAutoClear, classes }: SettingsTabProps) {
  const [autostartEnabled, setAutostartEnabled] = useState(false);

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
        </div>
      </div>
    </div>
  );
}

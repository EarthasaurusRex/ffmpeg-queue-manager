import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Field, Input, Textarea, Button, tokens } from "@fluentui/react-components";
import { Profile } from "../types";

interface ProfilesTabProps {
  profiles: Profile[];
  loadProfiles: () => Promise<void>;
  classes: any;
}

export default function ProfilesTab({ profiles, loadProfiles, classes }: ProfilesTabProps) {
  const [newProfileName, setNewProfileName] = useState("");
  const [newProfileArgs, setNewProfileArgs] = useState("-c:v libx264 -crf 23 -c:a aac");

  const handleSaveProfile = async () => {
    if (!newProfileName) return;
    try {
      const argsArray = newProfileArgs.match(/(?:[^\s"]+|"[^"]*")+/g)?.map(arg => arg.replace(/(^"|"$)/g, '')) || [];
      await invoke("save_profile", { name: newProfileName, args: argsArray, extension: null, suffix: null });
      setNewProfileName("");
      setNewProfileArgs("-c:v libx264 -crf 23 -c:a aac");
      loadProfiles();
    } catch (err) {
      alert("Error saving profile: " + err);
    }
  };

  const handleDeleteProfile = async (name: string) => {
    try {
      await invoke("delete_profile", { name });
      loadProfiles();
    } catch (err) {
      alert("Error deleting profile: " + err);
    }
  };

  const handleImportProfile = async () => {
    try {
      const selected = await open({
        multiple: true,
        filters: [{ name: "JSON", extensions: ["json"] }],
      });
      if (!selected) return;

      const files = Array.isArray(selected) ? selected : [selected];
      let imported = 0;

      for (const file of files) {
        const path = file;
        const contents = await invoke<string>("read_profile_file", { path });
        const parsed = JSON.parse(contents);
        
        if (parsed.name && Array.isArray(parsed.args)) {
          await invoke("save_profile", { 
            name: parsed.name, 
            args: parsed.args, 
            extension: parsed.extension || null, 
            suffix: parsed.suffix || null 
          });
          imported++;
        } else {
          console.error(`Invalid profile structure in ${path}`);
        }
      }
      
      if (imported > 0) {
        loadProfiles();
        alert(`Successfully imported ${imported} profile(s).`);
      }
    } catch (err) {
      alert("Error importing profiles: " + err);
    }
  };

  return (
    <div className={classes.profilesGrid}>
      <div className={classes.profileCard}>
        <h3>Create New Profile</h3>
        <Field label="Profile Name">
          <Input 
            value={newProfileName} 
            onChange={e => setNewProfileName(e.target.value)} 
            placeholder="e.g. Convert to MP4" 
          />
        </Field>
        <Field label="FFmpeg Arguments" style={{ marginTop: 10 }}>
          <Textarea 
            value={newProfileArgs} 
            onChange={e => setNewProfileArgs(e.target.value)} 
            placeholder="-c:v libx264 -crf 23 -c:a aac" 
          />
        </Field>
        <div style={{ marginTop: 10, display: "flex", gap: "10px" }}>
          <Button appearance="primary" onClick={handleSaveProfile}>
            Save Profile
          </Button>
          <Button appearance="secondary" onClick={handleImportProfile}>
            Import Profile(s)
          </Button>
        </div>
      </div>

      <div>
        <h3>Existing Profiles</h3>
        {profiles.length === 0 ? (
          <p>No profiles found.</p>
        ) : (
          profiles.map(p => (
            <div key={p.name} className={classes.profileCard} style={{ marginBottom: 10 }}>
              <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
                <strong>{p.name}</strong>
                <Button appearance="transparent" onClick={() => handleDeleteProfile(p.name)}>Delete</Button>
              </div>
              <code style={{ display: "block", marginTop: 5, padding: 5, backgroundColor: tokens.colorNeutralBackground3 }}>
                {p.args.join(" ")}
              </code>
            </div>
          ))
        )}
      </div>
    </div>
  );
}

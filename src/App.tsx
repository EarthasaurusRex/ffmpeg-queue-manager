import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Command, Child } from "@tauri-apps/plugin-shell";
import {
  makeStyles,
  tokens,
  Title2,
  TabList,
  Tab,
} from "@fluentui/react-components";

import { Job, JobStatus, Profile } from "./types";
import QueueTab from "./components/QueueTab";
import ProfilesTab from "./components/ProfilesTab";
import SettingsTab from "./components/SettingsTab";

const useStyles = makeStyles({
  container: {
    display: "flex",
    flexDirection: "column",
    height: "100vh",
    padding: "20px",
    boxSizing: "border-box",
    backgroundColor: tokens.colorNeutralBackground1,
    overflow: "hidden",
  },
  header: {
    display: "flex",
    justifyContent: "space-between",
    alignItems: "center",
    marginBottom: "10px",
  },
  content: {
    flexGrow: 1,
    overflowY: "auto",
    backgroundColor: tokens.colorNeutralBackground2,
    borderRadius: tokens.borderRadiusMedium,
    border: `1px solid ${tokens.colorNeutralStroke1}`,
    padding: "10px",
  },
  profilesGrid: {
    display: "flex",
    flexDirection: "column",
    gap: "20px",
  },
  profileCard: {
    padding: "15px",
    border: `1px solid ${tokens.colorNeutralStroke1}`,
    borderRadius: tokens.borderRadiusMedium,
    backgroundColor: tokens.colorNeutralBackground1,
  },
  logsContainer: {
    fontFamily: "monospace",
    whiteSpace: "pre-wrap",
    wordBreak: "break-all",
    backgroundColor: tokens.colorNeutralBackgroundInverted,
    color: tokens.colorNeutralForegroundInverted,
    padding: "10px",
    borderRadius: tokens.borderRadiusMedium,
    maxHeight: "300px",
    overflowY: "auto",
    fontSize: "12px",
    marginTop: "5px",
    marginBottom: "5px",
  },
  statsText: {
    fontSize: "0.8em",
    color: tokens.colorNeutralForeground3,
    marginTop: "4px",
  }
});

const formatTime = (secs: number) => {
  if (!isFinite(secs) || secs < 0) return "--:--";
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = Math.floor(secs % 60);
  return [h, m, s].map(v => v.toString().padStart(2, '0')).join(':');
};

export default function App() {
  const classes = useStyles();
  const [selectedTab, setSelectedTab] = useState<"queue" | "profiles" | "settings">("queue");
  
  // Settings State
  const [maxConcurrency, setMaxConcurrency] = useState(2);
  const [autoClear, setAutoClear] = useState(false);
  
  // Refs for Settings (to access inside async closure)
  const maxConcurrencyRef = useRef(maxConcurrency);
  const autoClearRef = useRef(autoClear);

  useEffect(() => { maxConcurrencyRef.current = maxConcurrency; }, [maxConcurrency]);
  useEffect(() => { autoClearRef.current = autoClear; }, [autoClear]);

  // Queue State
  const [jobs, setJobs] = useState<Job[]>([]);
  const jobsRef = useRef(jobs);
  const activeRunners = useRef(0);
  const activeProcesses = useRef<{ [jobId: string]: Child }>({});

  const stopJob = async (jobId: string) => {
    if (activeProcesses.current[jobId]) {
      try {
        await activeProcesses.current[jobId].kill();
      } catch (err) {
        console.error("Error killing process:", err);
      }
    }
    setJobs((prev) => prev.map((j) => j.id === jobId ? { ...j, status: "Error", progress: 0 } : j));
  };

  // Terminal Auto-scrollers
  const terminalRefs = useRef<{ [key: string]: HTMLDivElement | null }>({});

  // Profiles State
  const [profiles, setProfiles] = useState<Profile[]>([]);

  useEffect(() => {
    jobsRef.current = jobs;
  }, [jobs]);

  const loadProfiles = async () => {
    try {
      const data = await invoke<Profile[]>("get_profiles");
      setProfiles(data);
    } catch (err) {
      console.error(err);
    }
  };

  useEffect(() => {
    loadProfiles();
  }, []);

  // Listener for single-instance args (Queue Items)
  useEffect(() => {
    const unlisten = listen<string[]>("queue-items", (event) => {
      const args = event.payload;
      let profileName = "";
      const files: string[] = [];
      
      for (let i = 1; i < args.length; i++) {
        if (args[i] === "--profile" && i + 1 < args.length) {
          profileName = args[i + 1].replace(".json", "");
          i++; 
        } else if (!args[i].startsWith("--")) {
          files.push(args[i]);
        }
      }

      if (profileName && files.length > 0) {
        setJobs((prev) => {
          const newJobs = files.map((file, idx) => ({
            id: Date.now().toString() + idx + Math.random().toString(),
            filePath: file,
            profileName,
            status: "Queued" as JobStatus,
            progress: 0,
            logs: [],
            expandedTerminal: false,
          }));
          return [...prev, ...newJobs];
        });
      }
    });

    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const appendLog = (jobId: string, line: string) => {
    setJobs((prev) =>
      prev.map((j) => {
        if (j.id === jobId) {
          const newLogs = [...j.logs];
          const isFrameLine = line.startsWith("[FFMPEG] frame=");
          
          if (isFrameLine && newLogs.length > 0 && newLogs[newLogs.length - 1].startsWith("[FFMPEG] frame=")) {
            // Overwrite the last progress line to prevent terminal spam
            newLogs[newLogs.length - 1] = line;
          } else {
            newLogs.push(line);
            if (newLogs.length > 1000) newLogs.shift(); // Keep last 1000
          }
          
          // Auto-scroll
          setTimeout(() => {
            if (terminalRefs.current[jobId]) {
              terminalRefs.current[jobId]!.scrollTop = terminalRefs.current[jobId]!.scrollHeight;
            }
          }, 10);

          return { ...j, logs: newLogs };
        }
        return j;
      })
    );
  };

  // Job Runner Engine
  useEffect(() => {
    const runNext = async () => {
      if (activeRunners.current >= maxConcurrencyRef.current) return;

      const currentJobs = jobsRef.current;
      const nextJobIndex = currentJobs.findIndex((j) => j.status === "Queued");
      
      if (nextJobIndex === -1) return;

      const nextJob = currentJobs[nextJobIndex];
      
      setJobs((prev) =>
        prev.map((j) => (j.id === nextJob.id ? { ...j, status: "Running" } : j))
      );
      
      activeRunners.current += 1;

      try {
        appendLog(nextJob.id, `[SYSTEM] Started job for ${nextJob.filePath}`);
        
        // Find profile
        const allProfiles = await invoke<Profile[]>("get_profiles");
        const profile = allProfiles.find(p => p.name === nextJob.profileName);
        
        if (!profile) {
          throw new Error(`Profile '${nextJob.profileName}' not found`);
        }

        appendLog(nextJob.id, `[SYSTEM] Loaded profile '${profile.name}'`);

        // Run ffprobe to determine track count
        appendLog(nextJob.id, `[SYSTEM] Running ffprobe to detect audio tracks...`);
        const probeCmd = Command.sidecar("bin/ffprobe", [
          "-v", "error", 
          "-select_streams", "a", 
          "-show_entries", "stream=index", 
          "-of", "csv=p=0", 
          nextJob.filePath
        ]);

        probeCmd.stdout.on("data", (line) => appendLog(nextJob.id, `[FFPROBE] ${line}`));
        probeCmd.stderr.on("data", (line) => appendLog(nextJob.id, `[FFPROBE] ${line}`));

        const probeRes = await probeCmd.execute();
        if (probeRes.code !== 0) {
          throw new Error(`ffprobe failed: ${probeRes.stderr}`);
        }

        const acount = probeRes.stdout.split('\n').filter(l => l.trim().length > 0).length;
        appendLog(nextJob.id, `[SYSTEM] Detected ${acount} audio tracks`);

        const inputPath = nextJob.filePath;
        const ext = profile.extension || ".mp4";
        let resolvedPrefix = profile.prefix || "";
        let resolvedSuffix = profile.suffix || "_processed";
        resolvedSuffix = resolvedSuffix.replace("{{MASTER_RAW}}", acount > 1 ? " Master+Raw" : "");

        const lastSlashIdx = Math.max(inputPath.lastIndexOf('/'), inputPath.lastIndexOf('\\'));
        const dir = inputPath.substring(0, lastSlashIdx + 1);
        const filename = inputPath.substring(lastSlashIdx + 1);
        const basename = filename.replace(/\.[^/.]+$/, "");

        const outputPath = dir + resolvedPrefix + basename + resolvedSuffix + ext;
        
        let finalArgs: string[] = [];
        for (let i = 0; i < profile.args.length; i++) {
          const arg = profile.args[i];
          if (arg.includes("{{AUTO_AMIX}}") || arg.includes("{{AUTO_AMERGE_LOUDNORM}}")) {
            if (acount > 1) {
              let filterStr = "";
              for(let k=0; k<acount; k++) filterStr += `[0:a:${k}]`;
              if (arg.includes("{{AUTO_AMIX}}")) filterStr += `amix=inputs=${acount}:duration=longest[aout]`;
              else filterStr += `amerge=inputs=${acount},loudnorm[aout]`;
              finalArgs.push(filterStr);
            } else {
              if (finalArgs.length > 0 && finalArgs[finalArgs.length - 1] === "-filter_complex") {
                finalArgs.pop();
              }
            }
          } else if (arg === "[aout]") {
            if (acount > 1) {
              finalArgs.push(arg);
            } else {
              if (finalArgs.length > 0 && finalArgs[finalArgs.length - 1] === "-map") {
                finalArgs.pop();
              }
            }
          } else {
            finalArgs.push(arg);
          }
        }

        const cmdArgs = ["-y", "-i", inputPath, ...finalArgs, outputPath];
        appendLog(nextJob.id, `[SYSTEM] Launching FFmpeg with args: ${cmdArgs.join(" ")}`);
        
        const cmd = Command.sidecar("bin/ffmpeg", cmdArgs);
        
        let totalDurationSeconds = 0;

        cmd.stdout.on("data", (line) => appendLog(nextJob.id, `[FFMPEG] ${line}`));
        cmd.stderr.on("data", (chunk: string) => {
          const lines = chunk.split(/[\r\n]+/);
          
          for (const rawLine of lines) {
            const line = rawLine.trim();
            if (!line) continue;

            appendLog(nextJob.id, `[FFMPEG] ${line}`);
            
            // Parse duration
            const durationMatch = line.match(/Duration:\s+(\d{2}):(\d{2}):(\d{2}\.\d+)/);
            if (durationMatch) {
              totalDurationSeconds =
                parseInt(durationMatch[1]) * 3600 +
                parseInt(durationMatch[2]) * 60 +
                parseFloat(durationMatch[3]);
              appendLog(nextJob.id, `[DEBUG] PARSED DURATION: ${totalDurationSeconds}s`);
            }

            let fpsStr: string | undefined;
            let bitrateStr: string | undefined;
            let speedStr: string | undefined;
            let etaStr: string | undefined;
            let currentProgress = -1;

            const fpsMatch = line.match(/fps=\s*([\d.]+)/);
            if (fpsMatch) fpsStr = fpsMatch[1];

            const bitrateMatch = line.match(/bitrate=\s*([\d.]+\s*kbits\/s)/);
            if (bitrateMatch) bitrateStr = bitrateMatch[1];

            const speedMatch = line.match(/speed=\s*([\d.]+)x/);
            if (speedMatch) speedStr = speedMatch[1];

            const timeMatch = line.match(/time=(\d{2}):(\d{2}):(\d{2}\.\d+)/);
            if (timeMatch) {
               if (totalDurationSeconds > 0) {
                  const currentSeconds =
                    parseInt(timeMatch[1]) * 3600 +
                    parseInt(timeMatch[2]) * 60 +
                    parseFloat(timeMatch[3]);
                  
                  currentProgress = Math.min(100, Math.round((currentSeconds / totalDurationSeconds) * 100));

                  if (speedStr) {
                     const speedNum = parseFloat(speedStr);
                     if (speedNum > 0) {
                        const remainingSecs = (totalDurationSeconds - currentSeconds) / speedNum;
                        etaStr = formatTime(remainingSecs);
                     }
                  }
               } else {
                  appendLog(nextJob.id, `[DEBUG] time matched, but totalDurationSeconds is 0!`);
               }
            }

            // Update state if anything changed
            if (currentProgress >= 0 || fpsStr || bitrateStr || etaStr) {
              setJobs((prev) =>
                prev.map((j) => {
                  if (j.id === nextJob.id) {
                    return {
                      ...j,
                      progress: currentProgress >= 0 ? currentProgress : j.progress,
                      fps: fpsStr || j.fps,
                      bitrate: bitrateStr || j.bitrate,
                      eta: etaStr || j.eta
                    };
                  }
                  return j;
                })
              );
            }
          }
        });

        const code = await new Promise<number>((resolve) => {
           cmd.on("close", (data) => resolve(data.code || 0));
           cmd.on("error", (err) => {
               appendLog(nextJob.id, `[SYSTEM ERROR] Failed to run FFmpeg: ${err}`);
               resolve(-1);
           });
           cmd.spawn().then(child => {
             activeProcesses.current[nextJob.id] = child;
           }).catch((err) => {
               appendLog(nextJob.id, `[SYSTEM ERROR] Failed to spawn FFmpeg: ${err}`);
               resolve(-1);
           });
        });
        
        delete activeProcesses.current[nextJob.id];
        
        appendLog(nextJob.id, `[SYSTEM] Job finished with exit code: ${code}`);

        setJobs((prev) =>
          prev.map((j) =>
            j.id === nextJob.id
              ? {
                  ...j,
                  status: code === 0 ? "Completed" : "Error",
                  progress: code === 0 ? 100 : j.progress,
                }
              : j
          )
        );

        if (code === 0 && autoClearRef.current) {
          setTimeout(() => {
            setJobs((prev) => prev.filter((j) => j.id !== nextJob.id));
          }, 5000);
        }
      } catch (err) {
        console.error("Job execution failed", err);
        appendLog(nextJob.id, `[SYSTEM ERROR] ${String(err)}`);
        setJobs((prev) =>
          prev.map((j) =>
            j.id === nextJob.id
              ? { ...j, status: "Error" }
              : j
          )
        );
      } finally {
        activeRunners.current -= 1;
        setTimeout(runNext, 100);
      }
    };

    runNext();
  }, [jobs]);

  const handleGenerateShortcuts = async () => {
    try {
      const res = await invoke<string>("generate_shortcuts");
      alert(res);
    } catch (err) {
      alert("Error: " + err);
    }
  };

  return (
    <div className={classes.container}>
      <div className={classes.header}>
        <Title2>FFmpeg Queue Manager</Title2>
        <TabList
          selectedValue={selectedTab}
          onTabSelect={(_, data) => setSelectedTab(data.value as any)}
        >
          <Tab value="queue">Queue</Tab>
          <Tab value="profiles">Profiles</Tab>
          <Tab value="settings">Settings</Tab>
        </TabList>
      </div>

      <div className={classes.content}>
        {selectedTab === "queue" && (
          <QueueTab 
            jobs={jobs} 
            setJobs={setJobs} 
            stopJob={stopJob}
            terminalRefs={terminalRefs} 
            handleGenerateShortcuts={handleGenerateShortcuts} 
            classes={classes} 
          />
        )}
        {selectedTab === "profiles" && (
          <ProfilesTab 
            profiles={profiles} 
            loadProfiles={loadProfiles} 
            classes={classes} 
          />
        )}
        {selectedTab === "settings" && (
          <SettingsTab 
            maxConcurrency={maxConcurrency} 
            setMaxConcurrency={setMaxConcurrency} 
            autoClear={autoClear} 
            setAutoClear={setAutoClear} 
            classes={classes} 
          />
        )}
      </div>
    </div>
  );
}

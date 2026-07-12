export type JobStatus = "Queued" | "Running" | "Completed" | "Error";

export interface Job {
  id: string;
  filePath: string;
  profileName: string;
  status: JobStatus;
  progress: number; // 0 to 100
  logs: string[];
  expandedTerminal: boolean;
  fps?: string;
  bitrate?: string;
  eta?: string;
}

export interface Profile {
  name: string;
  args: string[];
  prefix?: string;
  extension?: string;
  suffix?: string;
}

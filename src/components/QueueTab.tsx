import {
  Button,
  Table,
  TableHeader,
  TableRow,
  TableHeaderCell,
  TableBody,
  TableCell,
  TableCellLayout,
  Badge,
  ProgressBar,
  Menu,
  MenuTrigger,
  MenuList,
  MenuItem,
  MenuPopover,
  tokens,
} from "@fluentui/react-components";
import { MoreHorizontal20Regular } from "@fluentui/react-icons";
import { Command } from "@tauri-apps/plugin-shell";
import { Job, JobStatus } from "../types";

interface QueueTabProps {
  jobs: Job[];
  setJobs: React.Dispatch<React.SetStateAction<Job[]>>;
  stopJob: (jobId: string) => Promise<void>;
  terminalRefs: React.MutableRefObject<{ [key: string]: HTMLDivElement | null }>;
  handleGenerateShortcuts: () => void;
  classes: any;
}

export default function QueueTab({ jobs, setJobs, stopJob, terminalRefs, handleGenerateShortcuts, classes }: QueueTabProps) {
  const toggleTerminal = (jobId: string) => {
    setJobs((prev) =>
      prev.map((j) => (j.id === jobId ? { ...j, expandedTerminal: !j.expandedTerminal } : j))
    );
  };

  const clearJob = (jobId: string) => {
    setJobs((prev) => prev.filter((j) => j.id !== jobId));
  };

  const getStatusBadge = (status: JobStatus) => {
    switch (status) {
      case "Queued": return <Badge color="subtle" appearance="outline">Queued</Badge>;
      case "Running": return <Badge color="brand" appearance="filled">Running</Badge>;
      case "Completed": return <Badge color="success" appearance="filled">Completed</Badge>;
      case "Error": return <Badge color="danger" appearance="filled">Error</Badge>;
    }
  };

  const showInExplorer = async (filePath: string) => {
    try {
      // Use cmd /c explorer /select, "C:\path\to\file"
      await Command.create("cmd", ["/c", "explorer", "/select,", filePath.replace(/\//g, '\\')]).execute();
    } catch (err) {
      console.error("Failed to open explorer", err);
    }
  };

  return (
    <>
      <div style={{ marginBottom: 10, display: "flex", justifyContent: "flex-end" }}>
        <Button appearance="primary" onClick={handleGenerateShortcuts}>
          Generate SendTo Shortcuts
        </Button>
      </div>
      <Table aria-label="Queue Table">
        <TableHeader>
          <TableRow>
            <TableHeaderCell>File</TableHeaderCell>
            <TableHeaderCell>Profile</TableHeaderCell>
            <TableHeaderCell>Status</TableHeaderCell>
            <TableHeaderCell>Progress</TableHeaderCell>
            <TableHeaderCell>Actions</TableHeaderCell>
          </TableRow>
        </TableHeader>
        <TableBody>
          {jobs.length === 0 ? (
            <TableRow>
              <TableCell colSpan={5}>
                <TableCellLayout style={{ justifyContent: "center", padding: "20px", color: tokens.colorNeutralForeground3 }}>
                  No jobs in queue. Use "Send To" context menu on files.
                </TableCellLayout>
              </TableCell>
            </TableRow>
          ) : (
            jobs.flatMap((job) => [
              <TableRow key={job.id}>
                <TableCell>
                  <TableCellLayout truncate title={job.filePath}>
                    {job.filePath.split("\\").pop()?.split("/").pop()}
                  </TableCellLayout>
                </TableCell>
                <TableCell>{job.profileName}</TableCell>
                <TableCell>{getStatusBadge(job.status)}</TableCell>
                <TableCell>
                  <ProgressBar 
                    value={
                      job.status === "Completed" ? 1 : 
                      job.status === "Error" ? 1 : 
                      job.status === "Queued" ? 0 : 
                      job.progress / 100
                    } 
                    color={
                      job.status === "Completed" ? "success" : 
                      job.status === "Error" ? "error" : 
                      "brand"
                    } 
                  />
                  {(job.status === "Running" || job.status === "Completed") && (
                    <div className={classes.statsText}>
                      {job.fps ? `${job.fps} fps • ` : ""}
                      {job.bitrate ? `${job.bitrate} • ` : ""}
                      {job.estSize ? `Est. ${job.estSize} • ` : ""}
                      ETA: {job.eta || "--:--"}
                    </div>
                  )}
                </TableCell>
                <TableCell>
                  <div style={{ display: "flex", gap: "5px" }}>
                    <Button size="small" onClick={() => toggleTerminal(job.id)}>
                      {job.expandedTerminal ? "Hide Logs" : "Logs"}
                    </Button>
                    {(job.status === "Running" || job.status === "Queued") && (
                      <Button size="small" appearance="subtle" style={{ color: tokens.colorPaletteRedForeground1 }} onClick={() => stopJob(job.id)}>
                        Stop
                      </Button>
                    )}
                    {(job.status === "Completed" || job.status === "Error") && (
                      <Button size="small" appearance="subtle" onClick={() => clearJob(job.id)}>
                        Clear
                      </Button>
                    )}
                    <Menu>
                      <MenuTrigger disableButtonEnhancement>
                        <Button size="small" appearance="subtle" icon={<MoreHorizontal20Regular />} />
                      </MenuTrigger>
                      <MenuPopover>
                        <MenuList>
                          <MenuItem onClick={() => showInExplorer(job.filePath)}>Show in Explorer</MenuItem>
                        </MenuList>
                      </MenuPopover>
                    </Menu>
                  </div>
                </TableCell>
              </TableRow>,
              job.expandedTerminal && (
                <TableRow key={job.id + "-terminal"}>
                  <TableCell colSpan={5} style={{ padding: 0 }}>
                    <div 
                      className={classes.logsContainer} 
                      ref={(el) => { terminalRefs.current[job.id] = el; }}
                    >
                      {job.logs.length === 0 ? "No logs yet..." : job.logs.map((log, i) => (
                        <div key={i}>{log}</div>
                      ))}
                    </div>
                  </TableCell>
                </TableRow>
              )
            ])
          )}
        </TableBody>
      </Table>
    </>
  );
}

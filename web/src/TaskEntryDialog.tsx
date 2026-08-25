import { LoaderCircle } from "lucide-react";
import { useEffect, useState } from "react";

import {
  continueBlockReason,
  gateFix,
  gateReason,
  preselect,
  selectable,
} from "./entry";
import type { DatasourceOption, EntryFix, EntryGuard } from "./entry";
import { Modal, ModalFooter } from "./ui";

export function TaskEntryDialog({
  guard,
  onClose,
  onFix,
  onContinue,
}: {
  guard: EntryGuard;
  onClose: () => void;
  onFix: (fix: EntryFix) => void;
  onContinue: (sourceDatasourceId: string, targetDatasourceId: string) => void;
}) {
  const [sourceDatasourceId, setSourceDatasourceId] = useState(() =>
    guard.kind === "open" ? preselect(guard.sources) : "",
  );
  const [targetDatasourceId, setTargetDatasourceId] = useState(() =>
    guard.kind === "open" ? preselect(guard.targets) : "",
  );

  useEffect(() => {
    if (guard.kind !== "open") {
      return;
    }
    setSourceDatasourceId((current) => preselect(guard.sources, current));
    setTargetDatasourceId((current) => preselect(guard.targets, current));
  }, [guard]);

  if (guard.kind === "loading") {
    return (
      <Modal title="检查新建任务条件" onClose={onClose} busy={false} narrow>
        <div className="modal-body entry-loading">
          <LoaderCircle className="is-spinning" size={18} aria-hidden="true" />
          正在检查数据源和目标端 Agent
        </div>
      </Modal>
    );
  }

  if (guard.kind === "blocked") {
    const fix = gateFix(guard.gate);
    return (
      <Modal title="暂时不能新建任务" onClose={onClose} busy={false} narrow>
        <div className="modal-body entry-blocked">
          <strong>{gateReason(guard.gate)}</strong>
          <span>补齐这项条件后，再从作业中心进入新建任务。</span>
        </div>
        <footer className="modal-footer">
          <button className="button is-ghost" type="button" onClick={onClose}>
            取消
          </button>
          <button className="button is-primary" type="button" onClick={() => onFix(fix)}>
            {fix === "agents" ? "前往目标端 Agent" : "前往数据源"}
          </button>
        </footer>
      </Modal>
    );
  }

  const blockReason = continueBlockReason(
    guard,
    sourceDatasourceId,
    targetDatasourceId,
  );
  return (
    <Modal title="选择本次任务的数据源" onClose={onClose} busy={false}>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (blockReason === null) {
            onContinue(sourceDatasourceId, targetDatasourceId);
          }
        }}
      >
        <div className="modal-body entry-form">
          <DatasourceChoices
            legend="源端数据源"
            options={guard.sources}
            value={sourceDatasourceId}
            onChange={setSourceDatasourceId}
          />
          <DatasourceChoices
            legend="目标端数据源"
            options={guard.targets}
            value={targetDatasourceId}
            onChange={setTargetDatasourceId}
          />
          {blockReason !== null && <p className="entry-reason">{blockReason}</p>}
        </div>
        <ModalFooter
          onClose={onClose}
          busy={false}
          submitLabel="进入向导"
          submitDisabled={blockReason !== null}
        />
      </form>
    </Modal>
  );
}

function DatasourceChoices({
  legend,
  options,
  value,
  onChange,
}: {
  legend: string;
  options: readonly DatasourceOption[];
  value: string;
  onChange: (datasourceId: string) => void;
}) {
  return (
    <fieldset className="entry-options">
      <legend>{legend}</legend>
      {options.map((option) => {
        const enabled = selectable(option);
        return (
          <label
            className={`entry-option ${value === option.datasource_id ? "is-selected" : ""} ${
              enabled ? "" : "is-disabled"
            }`}
            key={option.datasource_id}
          >
            <input
              type="radio"
              name={legend}
              value={option.datasource_id}
              checked={value === option.datasource_id}
              disabled={!enabled}
              onChange={() => onChange(option.datasource_id)}
            />
            <span className="entry-option-copy">
              <strong>{option.name}</strong>
              <span>{option.connection}</span>
            </span>
            <span className={`entry-agent-status ${agentStatusClass(option)}`}>
              {agentStatusLabel(option)}
            </span>
          </label>
        );
      })}
    </fieldset>
  );
}

function agentStatusLabel(option: DatasourceOption): string {
  if (option.agentStatus === null) {
    return "源端直连";
  }
  if (option.agentStatus === "online") {
    return `${option.agentName} · 在线`;
  }
  if (option.agentStatus === "mismatch") {
    return `${option.agentName} · 身份不符`;
  }
  return `${option.agentName} · 离线`;
}

function agentStatusClass(option: DatasourceOption): string {
  if (option.agentStatus === null) {
    return "";
  }
  return option.agentStatus === "online" ? "is-online" : "is-offline";
}

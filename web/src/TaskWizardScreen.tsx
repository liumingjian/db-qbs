import {
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  LoaderCircle,
  RefreshCw,
  Search,
  Trash2,
  X,
} from "lucide-react";
import { forwardRef, useEffect, useId, useImperativeHandle, useMemo, useRef, useState } from "react";
import type { FormEvent, ReactNode, RefObject } from "react";

import {
  checkTargetTable,
  fetchBuilderColumns,
  fetchBuilderDblinks,
  fetchBuilderSqlColumns,
  fetchBuilderTables,
  fetchTargetColumns,
  fetchTargetTables,
  generateBuilderSql,
  previewBuilderRows,
  previewErrorMessage,
} from "./api";
import type { BuilderColumn, BuilderSql, BuilderTable, PreviewResult } from "./api";
import { overlayOwnsKeyboard } from "./dialogFocus";
import { messageFrom } from "./errors";
import type { DatasourceOption } from "./entry";
import { ICON, UpsertNote } from "./components/DesignSystem";
import { HighlightedSql, HighlightedSqlInput, SqlEditor } from "./SqlEditor";
import { Modal } from "./ui";
import {
  apply,
  canAdvance,
  confirm,
  foldedSteps,
  leaving,
  leavingConfirmation,
  selectionBlocker,
  taskName,
  toSpec,
  view,
} from "./wizard";
import type {
  Change,
  ConfirmTargetCheck,
  Draft,
  Loss,
  RailEntry,
  Step,
  WriteView,
} from "./wizard";
import { WRITE_MODES } from "./writeMode";

export interface TaskWizardScreenProps {
  initial: Draft;
  onCancel: () => void;
  onSubmit: (draft: Draft, action: "start" | "save-only") => Promise<void>;
  /**
   * 草稿每变一次就回一次（UX 评审 P1-5）。调用方拿它存盘，好让离开向导不再等于丢掉。
   * 每一次按键都会调到——**调用方不要把它塞进 state**，否则整屏跟着重渲染。
   */
  onDraftChange?: (draft: Draft) => void;
  /** 人在离开确认框上选了「丢弃」。调用方清掉存的那一份。 */
  onDiscardDraft?: () => void;
  sourceOptions?: readonly DatasourceOption[];
  targetOptions?: readonly DatasourceOption[];
}

export interface TaskWizardScreenHandle {
  requestLeave: (proceed: () => void) => void;
}

type PendingConfirmation =
  | { kind: "change"; intent: Change; loses: Loss }
  | { kind: "leave"; intent: Change; loses: Loss; proceed: () => void };

export const TaskWizardScreen = forwardRef<TaskWizardScreenHandle, TaskWizardScreenProps>(function TaskWizardScreen(
  {
    initial,
    onCancel,
    onSubmit,
    onDraftChange,
    onDiscardDraft,
    sourceOptions = [],
    targetOptions = [],
  },
  ref,
) {
  const [draft, setDraft] = useState(initial);
  /*
   * 五步里的第 1 步「选择数据」（#245）。`wizard.ts` 的 `Step` 只有 1-4 且一个字不能改，
   * 所以它不是一个 Step，而是本地状态——`draft.step` 全程照旧。
   *
   * 初值**不看 `draft.step`**：落在第 1 步的草稿多半已经把数据选完了（从存盘恢复回来的、
   * 进来编辑一个已有任务的），那种草稿该直接开在映射步上。开在选择屏的是「还没挑源表、
   * 也没写 SQL」的草稿——问的正是 `selectionBlocker` 那句话。
   */
  const [isSelectingData, setIsSelectingData] = useState(() => selectionBlocker(initial) !== null);
  const draftRef = useRef(initial);
  const [pending, setPending] = useState<PendingConfirmation | null>(null);
  const [tables, setTables] = useState<BuilderTable[]>([]);
  /*
   * 清单要等一次请求才回来，但草稿里已经连着的那条现在就得摆出来：不然打开一份走
   * DBLINK 的任务时，数据源行先渲染成「没有 DBLINK」的样子，清单到了再凭空多出一个
   * 控件——条件出现的控件正是这套两栏布局的天敌（#249）。
   */
  const [dblinks, setDblinks] = useState<string[]>(
    () => (initial.spec.dblink === undefined ? [] : [initial.spec.dblink]),
  );
  const [targetTables, setTargetTables] = useState<string[]>([]);
  const [targetMetadataKey, setTargetMetadataKey] = useState<string | null>(null);
  const [sourceFilter, setSourceFilter] = useState("");
  const [targetFilter, setTargetFilter] = useState("");
  const [expandedOwners, setExpandedOwners] = useState<ReadonlySet<string>>(new Set());
  const [busy, setBusy] = useState<"tables" | "columns" | "target" | "preview" | "check" | "submit" | null>(null);
  const tableRequest = useRef(0);
  const sourceColumnRequest = useRef(0);
  const targetColumnRequest = useRef(0);
  const previewRequest = useRef(0);
  const targetCheckRequest = useRef(0);
  const [error, setError] = useState<string | null>(null);
  const [sql, setSql] = useState<BuilderSql | null>(null);
  const [sqlError, setSqlError] = useState<string | null>(null);
  const [checkError, setCheckError] = useState<string | null>(null);
  /** Escape 只在这块容器里算数（#242）。 */
  const containerRef = useRef<HTMLElement | null>(null);
  /** 换步时焦点要落到新那一步的标题上（#239）。 */
  const headingRef = useRef<HTMLHeadingElement | null>(null);
  const lastStep = useRef(initial.step);
  const [announcement, setAnnouncement] = useState("");
  /*
   * 缺表提示搬到了两栏容器之外的通栏一行（#249），目标表输入框靠 aria-describedby
   * 指过去——那段话因此需要一个稳定的 id，不能再是输入框旁边匿名的一句。
   * 标签也随之从「包着输入框」变成 htmlFor，所以输入框自己也要一个 id。
   */
  const targetMissingNoteId = useId();
  const targetTableInputId = useId();
  const model = view(draft);
  const advanceBlocked = model.step.blockers.length > 0;
  /** 第 1 步走不下去的理由，或者 `null`。 */
  const selectionRefusal = selectionBlocker(draft);
  /*
   * 五格轨道：第 1 格是「选择数据」那个本地状态，后四格是 `wizard.ts` 给的那四步。
   * 还在选择屏上时后四格一律 todo——那时 `draft.step` 虽然是 1，人却还没走到映射。
   *
   * `number` 就是人眼里的序号（见 `displayStep`），存进条目里而不是渲染时数下标：
   * 轨道要渲染两遍（左栏竖着、选择屏横着），下标算两次就是两个会各自漂的算法。
   */
  const railEntries: RailSlot[] = [
    { key: "selection", number: 1, label: "选择数据", state: isSelectingData ? "current" : "done" },
    ...model.rail.map((entry) => ({
      key: `step-${entry.step}`,
      number: displayStep(entry.step),
      label: entry.label,
      state: isSelectingData ? ("todo" as const) : entry.state,
    })),
  ];
  const stepCount = displayStepCount(model.rail);
  /*
   * 选择屏主按钮上写的必须是它**真正去的地方**。它做的只有一件事：离开选择屏——回到
   * `draft.step` 那一步。从第 3 步按「改 SQL」进来、又什么都没改的人，按下去回的是第 3 步，
   * 写死「下一步：选列」在那时候是句假话。真改了选择的话，`wizard.ts` 的清空规则会把
   * `draft.step` 拨回 1，这一句也就跟着变回「下一步：选列」——标签跟着草稿走，不猜。
   */
  const selectionNextLabel = draft.step === 1
    ? "下一步：选列"
    : `回到第 ${displayStep(draft.step)} 步：${model.rail.find((entry) => entry.step === draft.step)?.label ?? ""}`;
  /** 最后一步为什么提交不了，或者 `null`。 */
  const submitRefusal =
    busy === "submit" ? "正在提交" : canAdvance(draft, 4)[0]?.message ?? null;
  const sqlIdentity = JSON.stringify(toSpec(draft));

  function commit(next: Draft) {
    draftRef.current = next;
    setDraft(next);
    onDraftChange?.(next);
  }

  function requestChange(intent: Change) {
    const current = draftRef.current;
    const result = apply(current, intent);
    if (result.kind === "done") {
      commit(result.draft);
      return;
    }
    setPending({ kind: "change", intent: result.intent, loses: result.loses });
  }

  /** 离开向导一律先问一句（#242）；问什么、值不值得问，都是 `wizard.ts` 的规矩。 */
  function requestLeave(proceed: () => void) {
    const loses = leavingConfirmation(draftRef.current);
    setPending({ kind: "leave", intent: { type: "leave" }, loses, proceed });
  }

  function acceptPending() {
    if (pending === null) return;
    commit(confirm(draftRef.current, pending.intent));
    setPending(null);
    if (pending.kind === "leave") pending.proceed();
  }

  /** 「丢弃草稿并离开」——存的那一份也一起扔掉，不然下一屏又会摆出来。 */
  function discardAndLeave() {
    if (pending === null || pending.kind !== "leave") return;
    onDiscardDraft?.();
    setPending(null);
    pending.proceed();
  }

  useImperativeHandle(ref, () => ({ requestLeave }), []);

  /*
   * Escape 退出向导，但只在**向导容器里**按的那一下算数（#242）。
   *
   * 原来这个监听挂在 window 上，于是任何一下 Escape 都是「退出向导」——包括收起一个原生
   * `<select>` 的弹出层、或者关掉浏览器的自动填充下拉，这两样这一屏上都有。人以为自己
   * 关掉的是刚弹出来的那个东西，整屏没了。挂到容器上之后，容器外面按的 Escape 根本到不了
   * 这里；容器里开着浮层时，按键归最上面那一层管（`useDialogFocus` 自己排的队），
   * Escape 只收起那一层——全屏 SQL 编辑器为此额外拦一道捕获阶段的日子也就到头了。
   */
  useEffect(() => {
    const container = containerRef.current;
    if (container === null) return;
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape" || overlayOwnsKeyboard()) return;
      event.preventDefault();
      /*
       * 这一下 Escape 到这里就为止了，不再往上走。
       *
       * 少了这一句，按 Escape 的效果是**什么都没发生**：`requestLeave` 里的 setState 在
       * 原生事件里当场冲刷，确认框同步挂上，`useDialogFocus` 随即往 window 上装它自己的
       * 按键监听——而这同一个 Escape 才刚走到向导容器，还没冒泡到 window。于是它接着撞上
       * 那个刚装好的监听，被当成「收起最上面那一层」，确认框在同一次派发里又被关掉了。
       * 人看到的是一闪都没闪，Escape 像是坏的。
       *
       * 这不是把全屏编辑器那道捕获阶段的拦截换个地方装回来：那一道是抢在别人**之前**把
       * 事件掐掉，这一句是「这下按键我已经处理完了」，上面不该再有人拿同一下做第二件事。
       * 浮层当家时上面那个 `return` 先走，这一句根本轮不到。
       *
       * **这一句只在 `useDialogFocus` 把监听装在 window 上、且走冒泡阶段时才管用**
       * （见 `dialogFocus.ts`）：那样它才排在这块容器**后面**，才拦得住。哪天把那个
       * 监听改成捕获阶段，或者改挂到浮层自己的容器上，这一句就拦不到它了，Escape 会
       * 重新变成「一闪都没闪」——搬之前先回来读这一段。
       */
      event.stopPropagation();
      requestLeave(onCancel);
    }
    container.addEventListener("keydown", handleKeyDown);
    /*
     * 挂完监听先把焦点收进来。
     *
     * 不收的话，刚打开向导、还没按过 Tab 时焦点在 `<body>` 上，Escape 派发的目标在容器
     * 外面，压根到不了上面那个监听——退出向导这条路要先按一下 Tab 才通。收进来的是容器
     * 自己（`tabIndex={-1}`：能用脚本聚焦，不进 Tab 序），不抢任何一个控件，也不必假装
     * 换了一步；「容器外面按的 Escape 不退出向导」这条照旧成立，外面依然到不了这里。
     * 焦点已经在里面、或者上头压着一层浮层（进向导时的那个对话框）时，让开。
     */
    if (
      !overlayOwnsKeyboard() &&
      !(document.activeElement instanceof Node && container.contains(document.activeElement))
    ) {
      container.focus();
    }
    return () => container.removeEventListener("keydown", handleKeyDown);
    // requestLeave 只碰 ref 与 setState，身份变了也没有新东西可读。
    // 确认框自己就是队列里的一层，`pending` 因此不必再进依赖表。
  }, [onCancel]);

  useEffect(() => {
    if (leaving(draft) === null) return;
    function handleBeforeUnload(event: BeforeUnloadEvent) {
      event.preventDefault();
      event.returnValue = "";
    }
    window.addEventListener("beforeunload", handleBeforeUnload);
    return () => window.removeEventListener("beforeunload", handleBeforeUnload);
  }, [draft]);

  /*
   * 换了一步，焦点跟过去，顺带说一句到了第几步（#239）。
   *
   * 原来换步只改草稿：整块正文连同标题一起被换掉，焦点却还留在页脚那颗按钮上——
   * 它所属的内容已经不在了；读屏的人则什么都没听见。焦点落到新标题上是「同一屏内换步」
   * 的常规做法，不必假装发生了一次路由跳转。
   */
  useEffect(() => {
    if (lastStep.current === draft.step) return;
    const from = lastStep.current;
    lastStep.current = draft.step;
    headingRef.current?.focus();
    setAnnouncement(
      stepAnnouncement(model.rail, draft.step, foldedSteps(draft, from, draft.step)),
    );
    // model.rail 只随 draft.step 变，不必进依赖表。
  }, [draft.step]);

  /*
   * 进出选择屏也是一次换步：焦点跟过去，顺带说一句到了第几步（#239 的规矩，第 1 步照办）。
   * 头一次运行不算——那是刚打开向导，容器自己的初始焦点已经安排过了。
   */
  const firstRun = useRef(true);
  useEffect(() => {
    if (firstRun.current) {
      firstRun.current = false;
      return;
    }
    headingRef.current?.focus();
    setAnnouncement(
      isSelectingData
        ? `第 1 步，共 ${stepCount} 步：选择数据`
        : stepAnnouncement(model.rail, draft.step, []),
    );
    // model 只随 draft 变，进出选择屏时读的都是当下这一份。
  }, [isSelectingData]);

  async function loadTables() {
    const request = ++tableRequest.current;
    const current = draftRef.current;
    if (current.fetchMode !== "table") return;
    const datasourceId = current.source.datasource_id;
    const dblink = current.spec.dblink?.trim() ?? "";
    setBusy("tables");
    setError(null);
    try {
      const next = await fetchBuilderTables(datasourceId, dblink);
      const latest = draftRef.current;
      if (
        request !== tableRequest.current ||
        latest.fetchMode !== "table" ||
        latest.source.datasource_id !== datasourceId ||
        (latest.spec.dblink?.trim() ?? "") !== dblink
      ) return;
      setTables(next);
      const owners = new Set(next.map((table) => table.owner));
      if (owners.size === 1) setExpandedOwners(owners);
    } catch (loadError) {
      const latest = draftRef.current;
      if (
        request === tableRequest.current &&
        latest.fetchMode === "table" &&
        latest.source.datasource_id === datasourceId &&
        (latest.spec.dblink?.trim() ?? "") === dblink
      ) setError(messageFrom(loadError));
    } finally {
      if (request === tableRequest.current) {
        setBusy((currentBusy) => currentBusy === "tables" ? null : currentBusy);
      }
    }
  }

  async function loadSourceColumns() {
    const request = ++sourceColumnRequest.current;
    const current = draftRef.current;
    const datasourceId = current.source.datasource_id;
    const fetchMode = current.fetchMode;
    const dblink = current.spec.dblink?.trim() ?? "";
    const owner = current.spec.owner;
    const table = current.spec.table;
    const sourceSql = current.spec.source_sql?.trim() ?? "";
    if (fetchMode === "sql" ? sourceSql === "" : owner === "" || table === "") return;
    setBusy("columns");
    setError(null);
    try {
      let columns;
      if (fetchMode === "sql") {
        const fetched = await fetchBuilderSqlColumns({
          datasource_id: datasourceId,
          source_sql: sourceSql,
        });
        columns = fetched.map((column) => ({
          name: column.name,
          data_type: column.type,
          precision: column.precision,
          scale: column.scale,
          length: column.length,
          nullable: true,
        }));
      } else {
        columns = await fetchBuilderColumns({
          datasource_id: datasourceId,
          dblink,
          owner,
          table,
        });
      }
      const latest = draftRef.current;
      const stillCurrent =
        request === sourceColumnRequest.current &&
        latest.fetchMode === fetchMode &&
        latest.source.datasource_id === datasourceId &&
        (fetchMode === "sql"
          ? (latest.spec.source_sql?.trim() ?? "") === sourceSql
          : (latest.spec.dblink?.trim() ?? "") === dblink &&
            latest.spec.owner === owner && latest.spec.table === table);
      if (stillCurrent) {
        requestChange({ type: "source-columns-arrived", columns });
      }
    } catch (loadError) {
      const latest = draftRef.current;
      const stillCurrent =
        request === sourceColumnRequest.current &&
        latest.fetchMode === fetchMode &&
        latest.source.datasource_id === datasourceId &&
        (fetchMode === "sql"
          ? (latest.spec.source_sql?.trim() ?? "") === sourceSql
          : (latest.spec.dblink?.trim() ?? "") === dblink &&
            latest.spec.owner === owner && latest.spec.table === table);
      if (stillCurrent) setError(messageFrom(loadError));
    } finally {
      if (request === sourceColumnRequest.current) {
        setBusy((currentBusy) => currentBusy === "columns" ? null : currentBusy);
      }
    }
  }

  async function loadTarget() {
    const request = ++targetColumnRequest.current;
    const current = draftRef.current;
    const datasourceId = current.target.datasource_id;
    const table = current.spec.target_table;
    if (table.trim() === "") return;
    setBusy("target");
    setError(null);
    try {
      const metadata = await fetchTargetColumns(datasourceId, table);
      const latest = draftRef.current;
      if (
        request !== targetColumnRequest.current ||
        latest.target.datasource_id !== datasourceId ||
        latest.spec.target_table !== table
      ) {
        return;
      }
      setTargetMetadataKey(JSON.stringify([datasourceId, table]));
      requestChange({ type: "target-columns-arrived", columns: metadata.columns, keys: metadata.keys });
    } catch (loadError) {
      const latest = draftRef.current;
      if (
        request === targetColumnRequest.current &&
        latest.target.datasource_id === datasourceId &&
        latest.spec.target_table === table
      ) {
        setError(messageFrom(loadError));
      }
    } finally {
      if (request === targetColumnRequest.current) {
        setBusy((currentBusy) => currentBusy === "target" ? null : currentBusy);
      }
    }
  }

  function refreshTarget() {
    void reloadTargetTables();
    if (draftRef.current.spec.target_table.trim() === "") return;
    requestChange({ type: "refresh-target-columns" });
    void loadTarget();
  }

  async function reloadTargetTables() {
    try {
      setTargetTables(await fetchTargetTables(draftRef.current.target.datasource_id));
    } catch (loadError) {
      setError(messageFrom(loadError));
    }
  }

  async function loadPreview() {
    const request = ++previewRequest.current;
    const current = draftRef.current;
    const identity = previewIdentity(current);
    setBusy("preview");
    setError(null);
    try {
      const preview = await previewBuilderRows(
        current.source.datasource_id,
        toSpec(current),
        10,
      );
      if (request !== previewRequest.current || previewIdentity(draftRef.current) !== identity) {
        return;
      }
      requestChange({ type: "preview-arrived", preview });
    } catch (previewError) {
      if (request === previewRequest.current && previewIdentity(draftRef.current) === identity) {
        setError(previewErrorMessage(previewError));
      }
    } finally {
      if (request === previewRequest.current) {
        setBusy((currentBusy) => currentBusy === "preview" ? null : currentBusy);
      }
    }
  }

  async function loadCheck() {
    const request = ++targetCheckRequest.current;
    const current = draftRef.current;
    if (canAdvance(current, 1).length > 0) return;
    const inputs = JSON.stringify([
      current.source.datasource_id,
      current.target.datasource_id,
      current.spec.target_table,
      current.spec.columns,
      current.spec.primary_key,
    ]);
    setBusy("check");
    setCheckError(null);
    try {
      const result = await checkTargetTable(
        current.source.datasource_id,
        current.target.datasource_id,
        current.spec.target_table,
        toSpec(current),
      );
      const latest = draftRef.current;
      const latestInputs = JSON.stringify([
        latest.source.datasource_id,
        latest.target.datasource_id,
        latest.spec.target_table,
        latest.spec.columns,
        latest.spec.primary_key,
      ]);
      if (request === targetCheckRequest.current && inputs === latestInputs) {
        requestChange({ type: "check-arrived", check: result });
      }
    } catch (loadError) {
      if (request === targetCheckRequest.current) setCheckError(messageFrom(loadError));
    } finally {
      if (request === targetCheckRequest.current) {
        setBusy((currentBusy) => currentBusy === "check" ? null : currentBusy);
      }
    }
  }

  useEffect(() => {
    let active = true;
    void Promise.all([
      fetchBuilderDblinks(draft.source.datasource_id).catch(() => []),
      fetchTargetTables(draft.target.datasource_id),
    ])
      .then(([nextDblinks, nextTargets]) => {
        if (!active) return;
        setDblinks(nextDblinks);
        setTargetTables(nextTargets);
      })
      .catch((loadError) => active && setError(messageFrom(loadError)));
    return () => {
      active = false;
    };
  }, [draft.source.datasource_id, draft.target.datasource_id]);

  useEffect(() => {
    if (draft.fetchMode === "table") void loadTables();
    // The datasource and dblink are the table-list inputs.
  }, [draft.fetchMode, draft.source.datasource_id, draft.spec.dblink]);

  useEffect(() => {
    if (
      draft.step === 1 &&
      draft.fetchMode === "table" &&
      draft.spec.owner !== "" &&
      draft.spec.table !== ""
    ) {
      void loadSourceColumns();
    }
  }, [draft.step, draft.fetchMode, draft.source.datasource_id, draft.spec.dblink, draft.spec.owner, draft.spec.table]);

  useEffect(() => {
    if (
      draft.step !== 1 ||
      draft.fetchMode !== "sql" ||
      (draft.spec.source_sql?.trim() ?? "") === ""
    ) return;
    const timer = window.setTimeout(() => void loadSourceColumns(), 350);
    return () => window.clearTimeout(timer);
  }, [draft.step, draft.fetchMode, draft.source.datasource_id, draft.spec.source_sql]);

  useEffect(() => {
    if (
      (draft.step !== 1 && draft.step !== 3) ||
      draft.spec.target_table.trim() === ""
    ) return;
    // 目标表名现在是**打出来的**（P1-4），不再只能点。每敲一个字母去读一次目标库的
    // 元数据，等于把一次表名输入变成十几个请求——与自定义 SQL 那一路同一个节流。
    const timer = window.setTimeout(() => void loadTarget(), 350);
    return () => window.clearTimeout(timer);
  }, [draft.step, draft.target.datasource_id, draft.spec.target_table]);

  useEffect(() => {
    const expectedMetadata = JSON.stringify([
      draft.target.datasource_id,
      draft.spec.target_table,
    ]);
    if (
      draft.check === null &&
      targetMetadataKey === expectedMetadata &&
      canAdvance(draft, 1).length === 0
    ) {
      void loadCheck();
    }
  }, [draft, targetMetadataKey]);

  useEffect(() => {
    if (draft.step !== 2) {
      setSql(null);
      setSqlError(null);
      return;
    }
    let active = true;
    const spec = toSpec(draft);
    setSql(null);
    setSqlError(null);
    const timer = window.setTimeout(() => {
      void generateBuilderSql(spec)
        .then((next) => {
          if (active) {
            setSql(next);
            setSqlError(null);
          }
        })
        .catch((loadError) => active && setSqlError(messageFrom(loadError)));
    }, 250);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [draft.step, sqlIdentity]);

  const sourceGroups = useMemo(() => {
    const query = sourceFilter.trim().toLocaleLowerCase();
    const grouped = new Map<string, BuilderTable[]>();
    for (const table of tables) {
      if (`${table.owner}.${table.name}`.toLocaleLowerCase().includes(query)) {
        grouped.set(table.owner, [...(grouped.get(table.owner) ?? []), table]);
      }
    }
    return [...grouped.entries()];
  }, [sourceFilter, tables]);
  const filteredTargets = useMemo(() => {
    const query = targetFilter.trim().toLocaleLowerCase();
    return targetTables.filter((table) => table.toLocaleLowerCase().includes(query));
  }, [targetFilter, targetTables]);

  const sourceSqlEmpty = (draft.spec.source_sql ?? "").trim() === "";
  /** 已经查过、确认目标库里没有这张表。查之前不显示——那只是「还没查」。 */
  const targetTableMissing =
    draft.spec.target_table.trim() !== "" && !draft.targetTableExists;

  /*
   * 数据源行两侧同进同退（#249）。条件出现的行是这套两栏布局唯一的天敌：一侧多渲染
   * 一行，两栏当场差出一个 --ctl-h 加一个 gap。
   *
   * 所以「有没有这一行」由两侧合起来定（就是下面这一个判断），「这一行里放什么」才
   * 各自定——那一半归 `DatasourceRow`，两侧共用同一份。
   *
   * 两侧都空是另一回事：那说明压根没有部署信息（没登录、清单还没到），不是「只有一个
   * 数据源」，此时两侧一起不渲染，仍旧是齐的。
   */
  const showDatasourceRow = sourceOptions.length > 0 || targetOptions.length > 0;

  function selectTargetTable(table: string) {
    const alreadySelected = draftRef.current.spec.target_table === table;
    requestChange({ type: "target-table", table });
    if (alreadySelected) void loadTarget();
  }

  function advance() {
    requestChange({ type: "advance" });
  }

  /**
   * 步骤主体就是一张表单（#240）：在输入框里敲回车，等于按了这一步的主操作——
   * 选择数据与前三步是「下一步」，最后一步是「保存」或「开始导入」。这一步不让走时主按钮
   * 是禁用的，浏览器不会替禁用的默认按钮提交；这里再挡一道，回车便越不过拒绝理由。
   */
  function submitStep(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (isSelectingData) {
      if (selectionRefusal === null) setIsSelectingData(false);
      return;
    }
    if (draft.step < 4) {
      if (!advanceBlocked) advance();
      return;
    }
    if (submitRefusal === null) void submit(draft.mode === "edit" ? "save-only" : "start");
  }

  async function submit(action: "start" | "save-only") {
    setBusy("submit");
    setError(null);
    try {
      await onSubmit(draft, action);
    } catch (submitError) {
      setError(messageFrom(submitError));
      setBusy(null);
    }
  }

  /*
   * 第 1 步「选择数据」的源端卡片：一张卡画唯一那一层框，里面是编辑器或源表树。
   * 自定义 SQL 那一路把结果列收进同一张卡的下半段——它是这条 SQL 的产物，另起一块
   * 只会把左栏拉得比右栏长。
   */
  const sourceCard = draft.fetchMode === "sql" ? (
    <div className="wizard-pane-card">
      <SqlEditor
        value={draft.spec.source_sql ?? ""}
        placeholder="SELECT ID, NAME FROM APP.T_CUSTOMER"
        onChange={(value) => requestChange({ type: "source-sql", sql: value })}
        onFormat={(value) => requestChange({ type: "format-sql", sql: value })}
      />
      <div className="wizard-pane-result">
        <ResultColumns columns={draft.sourceColumns} busy={busy === "columns"} />
      </div>
    </div>
  ) : (
    <div className="wizard-pane-card">
      <label className="tree-search wizard-pane-card-head">
        <Search size={ICON.sm} aria-hidden="true" />
        <input value={sourceFilter} placeholder="筛选 owner / 表" onChange={(event) => setSourceFilter(event.target.value)} />
      </label>
      {/* **不声称是 tree**（UX 评审 P2）：`role="tree"` 承诺的是 treeitem 的整套键盘
          契约——上下左右移动、Home/End、展开折叠。这里是一组 `<button>`，一条都没实现，
          读屏软件照着 tree 的规则去用只会更糟。它实际上是一个「披露式分组列表」：
          owner 行自己带 aria-expanded。 */}
      <div className="schema-tree" role="group" aria-label="源表">
        {sourceGroups.map(([owner, ownerTables]) => {
          const open = expandedOwners.has(owner);
          return <div className="schema-node" key={owner}>
            <button className="schema-row" type="button" aria-expanded={open} onClick={() => setExpandedOwners((current) => {
              const next = new Set(current);
              if (open) next.delete(owner); else next.add(owner);
              return next;
            })}>
              {open ? <ChevronDown size={ICON.sm} /> : <ChevronRight size={ICON.sm} />}
              <span className="schema-name">{owner}</span><span className="schema-count">{ownerTables.length}</span>
            </button>
            {open && <div className="table-node-list">{ownerTables.map((table) => (
              <button
                className={`table-node ${draft.spec.owner === owner && draft.spec.table === table.name ? "is-selected" : ""}`}
                key={`${owner}.${table.name}`}
                type="button"
                onClick={() => requestChange({ type: "source-table", owner, table: table.name })}
              >{table.name}</button>
            ))}</div>}
          </div>;
        })}
      </div>
    </div>
  );

  /* 源端的控件行：报「现在选中的是什么」，刷新按钮就摆在它报的那个状态旁边——
     原来这颗刷新挂在面板表头里，离它刷新的东西隔着一整段。 */
  const sourceColbar = draft.fetchMode === "sql" ? (
    <div className="wizard-pane-colbar">
      <span className="wizard-pane-colbar-label">结果列</span>
      <div className="wizard-pane-colbar-row">
        <span className="wizard-pane-colbar-state">
          {draft.sourceColumns.length > 0
            ? `${draft.sourceColumns.length} 列`
            : busy === "columns" ? "正在识别…" : "尚未识别"}
        </span>
        {/* 理由不挂在这颗**自己就是 `disabled`** 的按钮的 `title` 上：一个字都显示不出来（#238）。 */}
        <Refusable reason={sourceSqlEmpty ? "先写好 SQL" : busy === "columns" ? "正在刷新结果列" : null}>{(describedBy) => (
          <button className="button" type="button" disabled={sourceSqlEmpty || busy === "columns"} aria-describedby={describedBy} onClick={() => void loadSourceColumns()}>
            {busy === "columns" ? <LoaderCircle className="is-spinning" size={ICON.sm} /> : <RefreshCw size={ICON.sm} />}刷新结果列
          </button>
        )}</Refusable>
      </div>
    </div>
  ) : (
    <div className="wizard-pane-colbar">
      <span className="wizard-pane-colbar-label">源表</span>
      <div className="wizard-pane-colbar-row">
        <span className="wizard-pane-colbar-state">
          {draft.spec.owner !== "" && draft.spec.table !== "" ? `${draft.spec.owner}.${draft.spec.table}` : "尚未选择"}
        </span>
        {/* 这一颗从不禁用，不必走 `Refusable`。 */}
        <button className="button" type="button" onClick={() => void loadTables()}>
          <RefreshCw className={busy === "tables" ? "is-spinning" : ""} size={ICON.sm} />刷新源表
        </button>
      </div>
    </div>
  );

  /* 第 1 步：占满主区全宽，源端 / 目标端左右对称两栏（#245）。 */
  const selectionBody = (
    <section className="wizard-step wizard-selection">
      <header className="wizard-selection-head">
        {/* tabIndex={-1}：能用脚本聚焦，但不进 Tab 序（#239）。全向导只此一个 `<h1>`。 */}
        <h1 ref={headingRef} tabIndex={-1}>选择数据</h1>
        <button className="icon-button" type="button" title="退出向导" aria-label="退出向导" onClick={() => requestLeave(onCancel)}><X size={ICON.md} /></button>
      </header>

      <div className="wizard-mode" role="group" aria-label="取数方式">
        <button
          type="button"
          className={draft.fetchMode === "table" ? "is-active" : ""}
          onClick={() => requestChange({ type: "fetch-mode", fetchMode: "table" })}
        >按表选择</button>
        <button
          type="button"
          className={draft.fetchMode === "sql" ? "is-active" : ""}
          onClick={() => requestChange({ type: "fetch-mode", fetchMode: "sql" })}
        >自定义 SQL</button>
      </div>

      <div className="wizard-panes">
        <section className="wizard-pane" aria-label="源端">
          {/* 表头只留「源端」：名字由下面的数据源行统一负责，不再重复一遍（#249）。 */}
          <header className="wizard-pane-head">
            <strong>源端</strong>
          </header>
          {/* 新建时也给（UX 评审 P1-10）：这两个下拉原来只在编辑态出现，于是新建路上
              选错了源库只能退回去从头再来一遍。改动本身有清空规则守着（Rule 1 / Rule 3），
              真会丢东西时向导自己会拦一道。 */}
          {showDatasourceRow && (
            <DatasourceRow
              label="源端数据源"
              options={sourceOptions}
              datasourceId={draft.source.datasource_id}
              boundName={model.context.sourceName}
              onPick={(option) => requestChange({ type: "source-datasource", datasource: option })}
            >
              {/* DBLINK 住在数据源这一行里（#249）：数据源吃满剩余宽度，DBLINK 定宽跟在
                  右边，没有 DBLINK 时数据源自然吃满，行高恒为 --ctl-h。
                  反例（不要写）：把 DBLINK 做成源端栏的又一个直接子元素——那是凭数据多出
                  一整行，源端卡片的上沿会比目标端低一截，正是这次要修的病。
                  仍旧走 `requestChange({ type: "dblink" })`：换 DBLINK 会清掉源表与映射
                  （`wizard.ts` 的清空规则 2），确认框由那条规则推出来，屏幕不自己判断。 */}
              {draft.fetchMode === "table" && dblinks.length > 0 && (
                <label className="wizard-select-label wizard-pane-ds-dblink">DBLINK
                  <select value={draft.spec.dblink ?? ""} onChange={(event) => requestChange({ type: "dblink", dblink: event.target.value })}>
                    <option value="">本地库</option>
                    {dblinks.map((dblink) => <option key={dblink}>{dblink}</option>)}
                  </select>
                </label>
              )}
            </DatasourceRow>
          )}
          {sourceColbar}
          {sourceCard}
        </section>

        <section className="wizard-pane" aria-label="目标端">
          <header className="wizard-pane-head">
            <strong>目标端</strong>
          </header>
          {/* 数据源行同进同退：两栏是**同一个组件**渲染的，不是抄成两份的两段长得像的
              JSX——两栏这一段严格同高，靠的正是这一点（CONTEXT.md 常设限制 #10）。 */}
          {showDatasourceRow && (
            <DatasourceRow
              label="目标端数据源"
              options={targetOptions}
              datasourceId={draft.target.datasource_id}
              boundName={model.context.targetName}
              onPick={(option) => requestChange({
                type: "target-datasource",
                datasource: option,
                online: option.agentStatus === "online",
              })}
            />
          )}
          {/* 目标端的控件行：与源端的控件行同结构同高（说明 + 一行 --ctl-h 的控件），
              刷新目标表这颗按钮从面板表头搬到了这里（#249）——刷新动作要和它刷新的那个
              状态在同一行，原来它挂在表头里，离目标表清单隔着一整段。
              目标表可以**直接打字**，不只能从清单里挑（UX 评审 P1-4）：打进一个还不存在
              的名字是正当用法，第 4 步会给出完整的 CREATE TABLE，自己执行完再回来刷新。
              产品不替人建表这条边界没动（CONTEXT.md）。 */}
          <div className="wizard-pane-colbar">
            <label className="wizard-pane-colbar-label" htmlFor={targetTableInputId}>目标表</label>
            <div className="wizard-pane-colbar-row">
              {/* 「尚不存在」徽标贴在输入框右端当锚点，绝对定位、不占独立行；解释那一段
                  在两栏容器之外，由 aria-describedby 连过去。留出的 78px 只在徽标真的
                  渲染时才让——表存在时长表名不该白丢一截字段宽度。 */}
              <span className={targetTableMissing ? "wizard-pane-target-input has-badge" : "wizard-pane-target-input"}>
                <input
                  id={targetTableInputId}
                  value={draft.spec.target_table}
                  placeholder="从下面挑一张，或直接输入表名"
                  spellCheck={false}
                  aria-describedby={targetTableMissing ? targetMissingNoteId : undefined}
                  onChange={(event) => requestChange({ type: "target-table", table: event.target.value })}
                />
                {targetTableMissing && <span className="field-badge is-inline wizard-pane-target-badge">尚不存在</span>}
              </span>
              {/* 刷新**不再绑在「已经选了表」上**（UX 评审 P1-4）：在别处刚建完表回来刷一下
                  清单，正是没选表的时候最需要的动作。选了表就顺带把它的列也重读一遍。 */}
              {/* 「正在刷新」是**按不动的理由**，挂在 title 上等于永远不显示（#238）：
                  按钮此刻正是禁用的。剩下那句是按得动时的提示，留在 title 上没问题。 */}
              <Refusable reason={busy === "target" ? "正在刷新" : null}>{(describedBy) => (
                <button className="icon-button" type="button" disabled={busy === "target"} title={busy === "target" ? undefined : draft.spec.target_table === "" ? "刷新目标表清单" : "刷新目标表清单与目标列"} aria-label="刷新目标表" aria-describedby={describedBy} onClick={refreshTarget}>
                  <RefreshCw className={busy === "target" ? "is-spinning" : ""} size={ICON.sm} />
                </button>
              )}</Refusable>
            </div>
          </div>
          <div className="wizard-pane-card">
            <label className="tree-search wizard-pane-card-head">
              <Search size={ICON.sm} aria-hidden="true" />
              <input value={targetFilter} placeholder="筛选目标表" onChange={(event) => setTargetFilter(event.target.value)} />
              <span className="tree-count">{filteredTargets.length} / {targetTables.length}</span>
            </label>
            {/* 目标表是**扁平的单选清单**，listbox / option 就是它本来的样子——
                这一档不用降级，直接说对即可，选中状态也因此报得出去。 */}
            <div className="schema-tree is-target" role="listbox" aria-label="目标表">
              {filteredTargets.map((table) => (
                <button
                  className={`table-node ${draft.spec.target_table === table ? "is-selected" : ""}`}
                  key={table}
                  type="button"
                  role="option"
                  aria-selected={draft.spec.target_table === table}
                  onClick={() => selectTargetTable(table)}
                >{table}</button>
              ))}
            </div>
          </div>
        </section>
      </div>

      {/* 通栏一行，落在 `.wizard-panes` **之外**（#249）：它出现或消失都不再动两栏的
          几何——原来它挤在目标端控件行与卡片之间，一命中就把目标端的卡片往下推两行
          中文的高度，两栏当场不齐。
          也不收进 title：#238 立的规矩是理由要是看得见的一行字。 */}
      {targetTableMissing && (
        <p className="wizard-missing-note" id={targetMissingNoteId}>
          <strong>{draft.spec.target_table}</strong> 这张表目标库里还没有。第 4 步会给出建表语句，运行前需要你自己执行。
        </p>
      )}
    </section>
  );

  return (
    <section
      className={`task-wizard ${isSelectingData ? "is-selection" : "is-steps"}`}
      ref={containerRef}
      /* 只为了让 Escape 一进来就有地方落（见上面那个监听），不进 Tab 序。 */
      tabIndex={-1}
      aria-label={draft.mode === "edit" ? "编辑任务" : "新建导入"}
    >
      {/* 选完数据之后左栏就收成这条：一条竖轨道 + 一小块只读摘要，再没有表单控件，
          也没有树——宽度因此让得出去，剩下的全给映射表。 */}
      {!isSelectingData && (
        <aside className="wizard-context">
          <header>
            <strong>{draft.mode === "edit" ? "编辑任务" : "新建导入"}</strong>
            <button className="icon-button" type="button" title="退出向导" aria-label="退出向导" onClick={() => requestLeave(onCancel)}><X size={ICON.md} /></button>
          </header>
          <div className="wizard-context-scroll">
            {/* 第 1 格可点，回去改源表 / 目标表 / SQL。 */}
            <WizardRail entries={railEntries} className="wizard-vrail" onEditSelection={() => setIsSelectingData(true)} />
            <div className="wizard-summary">
              <div className="wizard-summary-head">
                <strong>当前选择</strong>
                {/* 回第 1 步不必倒着走一遍向导（#245）：同一份草稿，换一屏改。 */}
                <button className="button wizard-summary-edit" type="button" onClick={() => setIsSelectingData(true)}>
                  {draft.fetchMode === "sql" ? "改 SQL" : "改选择"}
                </button>
              </div>
              <span>源端 · {model.context.sourceName}</span>
              <span className="mono">{model.context.sourceLabel}</span>
              <span>目标端 · {model.context.targetName}</span>
              <span className="mono">{model.context.targetTable}</span>
            </div>
          </div>
        </aside>
      )}

      <form className="wizard-main" noValidate onSubmit={submitStep}>
        {/* 第 1 步是全宽的，轨道横在顶上；之后轨道搬进左栏，这里不再重复一遍。 */}
        {isSelectingData && (
          <WizardRail entries={railEntries} className="wizard-rail" />
        )}

        {/* 全向导只此一处播报口（#239）：换到第几步、叫什么名字，以及向导替你折掉了哪一步——
            不然轨道上那一格自己打了勾，人是不知道为什么的。不是错误，所以用 role="status"。 */}
        <div className="wizard-live visually-hidden" role="status" aria-live="polite">{announcement}</div>

        <div className="wizard-step-scroll">
          {isSelectingData ? selectionBody : (
            <StepBody
              headingRef={headingRef}
              editSelection={() => setIsSelectingData(true)}
              draft={draft}
              sql={sql}
              sqlError={sqlError}
              checkError={checkError}
              busy={busy}
              change={requestChange}
              loadPreview={() => void loadPreview()}
              loadCheck={() => void loadCheck()}
            />
          )}
          {error !== null && <div className="form-error" role="alert">{error}</div>}
        </div>

        <footer className="wizard-footer">
          {/* 映射步的左键仍旧是「取消」（#240 的回车走查在这一步上量）：回第 1 步的路
              在左栏那颗「改 SQL」上，不占页脚这一格。 */}
          <button className="button" type="button" onClick={isSelectingData || draft.step === 1 ? () => requestLeave(onCancel) : () => requestChange({ type: "back" })}>
            {isSelectingData || draft.step === 1 ? "取消" : "上一步"}
          </button>
          <span className="wizard-footer-actions">
            {isSelectingData ? (
              <Refusable reason={selectionRefusal}>{(describedBy) => (
                <button className="button is-primary" type="submit" disabled={selectionRefusal !== null} aria-describedby={describedBy}>
                  {selectionNextLabel}
                </button>
              )}</Refusable>
            ) : draft.step < 4 ? (
              <Refusable reason={advanceBlocked ? "请先处理当前步骤中的问题" : null}>{(describedBy) => (
                <button className="button is-primary" type="submit" disabled={advanceBlocked} aria-describedby={describedBy}>
                  {draft.step === 3 ? "查看确认页" : "下一步"}
                </button>
              )}</Refusable>
            ) : draft.mode === "edit" ? (
              <Refusable reason={submitRefusal}>{(describedBy) => (
                <button className="button is-primary" type="submit" disabled={submitRefusal !== null} aria-describedby={describedBy}>
                  {busy === "submit" ? <LoaderCircle className="is-spinning" size={ICON.sm} /> : null}保存
                </button>
              )}</Refusable>
            ) : (
              <>
                <Refusable reason={submitRefusal}>{(describedBy) => (
                  <button className="button" type="button" disabled={submitRefusal !== null} aria-describedby={describedBy} onClick={() => void submit("save-only")}>只保存</button>
                )}</Refusable>
                <Refusable reason={submitRefusal}>{(describedBy) => (
                  <button className="button is-primary" type="submit" disabled={submitRefusal !== null} aria-describedby={describedBy}>
                    {busy === "submit" ? <LoaderCircle className="is-spinning" size={ICON.sm} /> : null}开始导入
                  </button>
                )}</Refusable>
              </>
            )}
          </span>
        </footer>
      </form>
      {pending !== null && (
        <WizardConfirmDialog
          loss={pending.loses}
          leaving={pending.kind === "leave"}
          onCancel={() => setPending(null)}
          onConfirm={acceptPending}
          onDiscard={discardAndLeave}
        />
      )}
    </section>
  );
});

/**
 * 两种确认共用这一个框：**一个改动会清掉什么**，和**离开时草稿里有什么**。
 *
 * 离开那一档在 UX 评审 P1-5 之后换了性质：草稿会存进 sessionStorage，所以它不再是
 * 「你要丢掉这些东西吗」，而是「这些东西会留着，你要现在走吗」。主按钮因此叫
 * **保留草稿并离开**，旁边多一颗真要扔掉的路——不给的话，那份草稿只能靠作业中心上
 * 那条通知去扔，而人此刻就在这里，就是在做这个决定。
 */
/**
 * 按不动的按钮**自己不会解释自己**：浏览器不给 `disabled` 控件派发指针事件，挂在控件
 * 上的 `title` 一个字都不会显示（UX 评审 P1-11）；挂到外层 `<span>` 上也只救得了鼠标——
 * 禁用的按钮不在 Tab 序里，只用键盘的人永远碰不到那句解释（#238）。
 *
 * 所以理由不再是提示气泡，而是**控件旁边的一行可见文字**，再用 `aria-describedby`
 * 认到控件头上，当它的可访问描述。控件可以继续禁用：理由不再依赖「够得着它」。
 */
function Refusable({ reason, children }: {
  reason: string | null;
  children: (describedBy: string | undefined) => ReactNode;
}) {
  const reasonId = useId();
  return reason === null ? <>{children(undefined)}</> : (
    <span className="refusal">
      {children(reasonId)}
      <small className="refusal-reason" id={reasonId}>{reason}</small>
    </span>
  );
}

export function WizardConfirmDialog({ loss, leaving = false, onCancel, onConfirm, onDiscard }: {
  loss: Loss;
  leaving?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
  onDiscard?: () => void;
}) {
  return <Modal title={loss.headline} onClose={onCancel} busy={false} narrow>
    <div className="modal-body wizard-loss">
      {loss.lines.length === 0
        ? <p>这份还没保存的草稿会留着，回来接着改。</p>
        : <ul>{loss.lines.map((line) => <li key={line}>{line}</li>)}</ul>}
    </div>
    <footer className="modal-footer">
      <button className="button is-ghost" type="button" onClick={onCancel}>取消</button>
      {leaving && onDiscard !== undefined && (
        <button className="button is-danger" type="button" onClick={onDiscard}>丢弃草稿并离开</button>
      )}
      <button className="button is-primary" type="button" onClick={onConfirm}>
        {leaving ? "保留草稿并离开" : "确认并继续"}
      </button>
    </footer>
  </Modal>;
}

/**
 * 「名字 · 连接串」，两侧都这么写。
 *
 * 清单里找不到这一侧时（另一侧有清单、这一侧没有——正是这一行存在的理由），`Draft` 里
 * 只存着名字，连接串这个进程压根没拿到。那就**说出来**：退回只报名字，两栏看着都是
 * 一行字，人却读不出这两行说的详略根本不同。
 */
function boundLabel(
  options: readonly DatasourceOption[],
  datasourceId: string,
  boundName: string,
): string {
  const bound = options.find((candidate) => candidate.datasource_id === datasourceId);
  return bound === undefined ? `${boundName} · 连接串未知` : `${bound.name} · ${bound.connection}`;
}

/**
 * 数据源那一行，源端目标端**同一个组件**（#249）。
 *
 * 抄成两份也能跑，但两栏严格同高就成了「两处记得一起改」——而这套两栏布局唯一的天敌
 * 正是某一栏比另一栏多出或少掉一行（CONTEXT.md 常设限制 #10）。行高、渲染成下拉还是
 * 只读文本、什么时候整行不出现，都只有一份实现。
 *
 * 可选项多于一个给下拉；只有一个、或这一侧压根没拿到清单，给标签 + 一行只读文本，
 * 两者同高。不用 disabled 下拉——那看着像坏了；也不留空占位块——那是给读屏软件的一段空洞。
 */
function DatasourceRow({ label, options, datasourceId, boundName, onPick, children }: {
  label: string;
  options: readonly DatasourceOption[];
  datasourceId: string;
  /** `Draft` 里存着的那个名字，清单里没有这一侧时的兜底。 */
  boundName: string;
  onPick: (option: DatasourceOption) => void;
  /** 跟在数据源右边的定宽控件（今天只有源端的 DBLINK）。 */
  children?: ReactNode;
}) {
  return <div className="wizard-pane-dsrow">
    {options.length > 1 ? (
      <label className="wizard-select-label wizard-pane-ds-main">{label}
        <select value={datasourceId} onChange={(event) => {
          const option = options.find((candidate) => candidate.datasource_id === event.target.value);
          if (option !== undefined) onPick(option);
        }}>
          {options.map((option) => <option key={option.datasource_id} value={option.datasource_id}>{option.name} · {option.connection}</option>)}
        </select>
      </label>
    ) : (
      <div className="wizard-select-label wizard-pane-ds-main">{label}
        <p className="wizard-pane-ds-fixed">{boundLabel(options, datasourceId, boundName)}</p>
      </div>
    )}
    {children}
  </div>;
}

/*
 * 五步里的第 1 格「选择数据」不是 `wizard.ts` 的 `Step`（`Step` 仍旧只有 1-4，#245 一个字
 * 没改），可人眼里它就是第 1 步。于是**显示的步号 = wizard 的步号 + 1**，**一共几步 =
 * 轨道长度 + 1**。这个 +1 原来散在七处（两处轨道下标、播报词里三处、CSS 的 repeat）——
 * 一样的意思写七遍，下次多一步就得七处都想起来。给它一个名字，改一处就够。
 * 唯一改不到的是 `app.css` 里 `.wizard-rail` 的 `repeat(5, …)`，那条自己带着注释指回来。
 */
const SELECTION_SLOT = 1;

/** 轨道上的一格：`number` 是人眼里的序号，不是 `Step`。 */
interface RailSlot {
  key: string;
  number: number;
  label: string;
  state: RailEntry["state"];
}

/** `wizard.ts` 的第 n 步在人眼里是第几步。 */
function displayStep(step: Step): number {
  return step + SELECTION_SLOT;
}

/** 人眼里这个向导一共几步。 */
function displayStepCount(rail: readonly RailEntry[]): number {
  return rail.length + SELECTION_SLOT;
}

/**
 * 步骤条，两处共用（左栏竖着一条、选择屏顶上横着一条）。
 *
 * 手抄两份的代价不是重复本身，是两份会各自漂：序号怎么算、走过的那一格画什么、
 * `aria-current` 落在谁头上——这些是**同一条轨道**的规矩，只能有一份。
 * 两处的差别只有壳的类名，和第 1 格是不是一颗能点回去的按钮。
 */
function WizardRail({ entries, className, onEditSelection }: {
  entries: readonly RailSlot[];
  className: string;
  /** 给了就把第 1 格渲染成「回去改选择」的按钮；选择屏自己在第 1 格上，不需要。 */
  onEditSelection?: () => void;
}) {
  return <ol className={className} aria-label="导入步骤">
    {entries.map((entry) => (
      <li className={`is-${entry.state}`} aria-current={entry.state === "current" ? "step" : undefined} key={entry.key}>
        {/* 走过的步子打勾、当前那一步填实心（UX 评审 P1-7）：两者原来长得一模一样，
            于是这条轨道说不出「我在哪儿」——而那正是它唯一的职责。 */}
        <span>{entry.state === "done" ? <Check size={ICON.sm} aria-label="已完成" /> : entry.number}</span>
        {onEditSelection !== undefined && entry.number === 1
          ? <button className="wizard-rail-back" type="button" onClick={onEditSelection}>{entry.label}</button>
          : <strong>{entry.label}</strong>}
      </li>
    ))}
  </ol>;
}

/**
 * 播报词（#239）：到了第几步、这一步叫什么，以及路上被折掉的那些步。
 *
 * 折掉哪几步不在这里数：中间隔着一步不等于向导跳过了它——往回走更不是。
 * 那是 `foldedSteps()` 照着真正的折叠信号（`checkIsSilent`）给的答案，这里只负责念出来。
 */
function stepAnnouncement(
  rail: readonly RailEntry[],
  to: Step,
  folded: readonly Step[],
): string {
  const label = (step: Step) => rail.find((entry) => entry.step === step)?.label ?? "";
  const skipped = folded.map((step) => `第 ${displayStep(step)} 步「${label(step)}」无需处理，已跳过。`);
  return `${skipped.join("")}第 ${displayStep(to)} 步，共 ${displayStepCount(rail)} 步：${label(to)}`;
}

function StepBody({
  headingRef,
  editSelection,
  draft,
  sql,
  sqlError,
  checkError,
  busy,
  change,
  loadPreview,
  loadCheck,
}: {
  headingRef: RefObject<HTMLHeadingElement | null>;
  /** 回第 1 步接着改源端。 */
  editSelection: () => void;
  draft: Draft;
  sql: BuilderSql | null;
  sqlError: string | null;
  checkError: string | null;
  busy: string | null;
  change: (intent: Change) => void;
  loadPreview: () => void;
  loadCheck: () => void;
}) {
  const model = view(draft).step;
  if (model.step === 1) {
    return <section className="wizard-step">
      <header>{/* tabIndex={-1}：能用脚本聚焦，但不进 Tab 序（#239）。 */}<h1 ref={headingRef} tabIndex={-1}>选列与字段映射</h1></header>
      {draft.fetchMode === "sql" && (
        /* SQL 正文归第 1 步（#245）。这里只留一小块只读回显——同一份 SQL 两处都能改，
           人会搞不清哪个算数。要改就回第 1 步改，草稿一个字都不丢。 */
        <section className="generated-sql wizard-sql-echo">
          <header>
            <div><strong>自定义 SQL</strong></div>
            <button className="button" type="button" onClick={editSelection}>改 SQL</button>
          </header>
          <pre className="ddl-output"><HighlightedSql sql={sqlEcho(draft.spec.source_sql ?? "")} /></pre>
        </section>
      )}
      {/* 写入模式就在这里，紧挨着下面那张表的「主键」一列（#261）。
          规格原话是「放在挑目标表与定主键的那一步」，而**向导里没有这样一步**：
          目标表在进四步之前的选择屏上挑，主键在第 1 步这张映射表里勾。两个决定
          里更该并排做的是「写入模式」与「主键」——它们一起决定了写出去的是哪条语句。
          目标表那一屏离它有一整个步骤的距离，把模式摆到那里，人会在还没有列清单、
          因而还看不见主键的时候先选写法。 */}
      <WriteModeCard write={model.write} change={change} />
      {model.rows.length === 0 ? <p className="wizard-empty">{draft.fetchMode === "sql" ? "尚未识别结果列" : "未选择源表"}</p> : (
        <div className="table-wrap"><table className="data-grid wizard-mapping"><thead><tr><th>同步</th><th>源列</th><th>目标列</th><th>主键</th><th><span className="visually-hidden">操作</span></th></tr></thead><tbody>
          {model.rows.map((row) => <tr className={row.problem ? "is-problem" : ""} key={row.source}>
            <td><input type="checkbox" aria-label={`同步 ${row.source}`} checked={row.selected} onChange={() => change({ type: "toggle-column", source: row.source })} /></td>
            <td><span className="mono">{row.source}</span></td>
            <td>{!row.selected ? "—" : <>{
              row.control === "auto" ? <span>{row.target} <small className="auto-mark">自动匹配</small></span>
              /* 表还不存在时没有列清单可挑，这一格里打的名字就是建表语句里的列名。 */
              : row.control === "new" ? <span className="new-target-field"><input aria-label={`${row.source} 的目标列名`} aria-invalid={row.problem ? true : undefined} value={row.target} spellCheck={false} onChange={(event) => change({ type: "rename-target", source: row.source, target: event.target.value })} /><small className="new-mark">将新建</small></span>
              : <select aria-label={`${row.source} 的目标列`} aria-invalid={row.problem ? true : undefined} value={row.target} onChange={(event) => change({ type: "rename-target", source: row.source, target: event.target.value })}><option value="">请选择</option>{draft.targetColumns.map((column) => <option key={column.name}>{column.name}</option>)}</select>
            }{row.problem && <small>{row.problem}</small>}</>}</td>
            {/* 主键锁定的那句话本来就是可见的，另外两句却只在 `title` 里——而这颗勾选框
                三种情况下都是 `disabled`，`title` 谁都看不到（#238）。三句合到同一条路上。 */}
            <td><Refusable reason={!row.selected ? "先勾选这一列" : row.target === "" ? "先选择目标列" : row.primaryKeyLock}>{(describedBy) => (
              <input type="checkbox" aria-label={`${row.source} 设为主键`} disabled={!row.selected || row.target === "" || row.primaryKeyLock !== null} aria-describedby={describedBy} checked={row.primaryKey} onChange={() => change({ type: "toggle-primary-key", target: row.target })} />
            )}</Refusable></td>
            <td><button className="icon-button is-danger" type="button" title={`删除列 ${row.source}`} aria-label={`删除列 ${row.source}`} onClick={() => change({ type: "remove-column", source: row.source })}><Trash2 size={ICON.sm} /></button></td>
          </tr>)}
        </tbody></table></div>
      )}
      <Blockers blockers={model.blockers} label="映射与主键" />
    </section>;
  }
  if (model.step === 2) {
    return <section className="wizard-step">
      <header><h1 ref={headingRef} tabIndex={-1}>过滤与验证</h1></header>
      {/* 这一格原来是个没有边框的 padding 容器，于是它的文本框和下面「构建 SQL」的卡
          边线各自差 12px。改成和邻居同规格的卡：表头把「这里只写条件、不写 WHERE」
          这句原来只在 aria-label 里的话摆到明面上。 */}
      {model.whereEditable ? <section className="where-clause-card"><header><div><strong>WHERE 条件</strong><span>只写条件本身，不用写 WHERE 关键字</span></div></header><div className="where-clause-editor"><HighlightedSqlInput value={model.where} placeholder="STATUS = 'ACTIVE' AND CREATED_AT >= DATE '2026-01-01'" label="WHERE 条件" rows={5} onChange={(clause) => change({ type: "where", clause })} /></div></section> : <div className="wizard-readonly">过滤条件写在自定义 SQL 中</div>}
      <section className="generated-sql"><header><div><strong>构建 SQL</strong></div></header>{sqlError ? <div className="form-error">{sqlError}</div> : sql ? <pre className="ddl-output"><HighlightedSql sql={sql.source_sql} /></pre> : <p className="spec-empty">正在生成最终查询。</p>}</section>
      <section className="preview-panel">
        <header><div><strong>数据预览</strong></div><button className="button" type="button" disabled={busy === "preview"} onClick={loadPreview}>{busy === "preview" ? <LoaderCircle className="is-spinning" size={ICON.sm} /> : null}预览前 10 条</button></header>
        {model.preview.value ? <PreviewData preview={model.preview.value} /> : <p className="spec-empty">尚未预览</p>}
      </section>
      <Blockers blockers={model.blockers} />
    </section>;
  }
  if (model.step === 3) {
    const result = model.check.value;
    // 表根本不存在时，findings 会是「每一列都缺」——那不是一张要逐条读的清单，
    // 那是一句话：这张表还没建。这一档只摆建表语句（UX 评审 P1-4）。
    const missing = !draft.targetTableExists && draft.spec.target_table.trim() !== "";
    return <section className="wizard-step">
      <header><h1 ref={headingRef} tabIndex={-1}>目标表检查</h1></header>
      <div className="target-check-toolbar">
        {/* 「目标表需要处理」原来是**兜底**：`result` 还是 null（一次都没查过）时
            也写这句，等于把「不知道」说成了「有问题」（UX 评审 P2）。 */}
        <strong>{
          busy === "check" ? "正在检查目标表"
          : missing ? "目标表尚不存在"
          : result === null ? "尚未检查目标表"
          : result.ok ? "目标表检查通过"
          : `目标表需要处理（${result.findings.length} 项）`
        }</strong>
        <button className="button" type="button" disabled={busy === "check" || busy === "target"} onClick={loadCheck}>
          <RefreshCw className={busy === "check" ? "is-spinning" : ""} size={ICON.sm} />重新检查
        </button>
      </div>
      {checkError !== null && <div className="form-error" role="alert">{checkError}</div>}
      {model.check.state === "stale" && <div className="form-error" role="alert">映射或主键已变化，请重新检查目标表。</div>}
      {model.check.state === "none" && busy !== "check" && checkError === null && <p className="wizard-empty">等待目标表元数据与字段映射就绪。</p>}
      {/* 预检结论：不是 finding，检查是**通过**的（#261）。通过之后还有一句话必须被读到。 */}
      {(result?.notes ?? []).map((note) => <UpsertNote key={note} text={note} />)}
      {missing && <p className="wizard-placeholder"><strong>{draft.spec.target_table} 在目标库里还没有</strong>用下面这条语句建好它，再点「重新检查」。</p>}
      {result !== null && !result.ok && <>
        {!missing && <div className="target-check-findings">
          {result.findings.map((finding, index) => <article key={`${finding.kind}-${finding.column ?? "table"}-${index}`}>
            <header><strong>{finding.column ?? "目标表"}</strong><span>{finding.message}</span></header>
            <dl><div><dt>需要</dt><dd>{finding.expected}</dd></div><div><dt>当前</dt><dd>{finding.actual}</dd></div></dl>
          </article>)}
        </div>}
        {result.suggested_ddl !== null && <section className="generated-sql target-check-ddl">
          <header><div><strong>{missing ? "建表语句" : "建议建表语句"}</strong><span>完整 CREATE TABLE</span></div><button className="icon-button" type="button" title="复制建表语句" aria-label="复制建表语句" onClick={() => void navigator.clipboard.writeText(result.suggested_ddl ?? "")}><Copy size={ICON.sm} /></button></header>
          <pre className="ddl-output"><HighlightedSql sql={result.suggested_ddl} /></pre>
        </section>}
      </>}
      <Blockers blockers={model.blockers} />
    </section>;
  }
  const confirmView = model.confirm;
  /* 勾选之外的源列——**别处没有一处说过它们**。第 1 步是一张勾选表，看得见的是「勾了什么」；
     没勾的那些只在这里被点名一次，而它们恰恰是跑完之后才想起来的那批。 */
  const carried = new Set(confirmView.mappings.map((mapping) => mapping.source));
  const dropped = draft.sourceColumns.filter((column) => !carried.has(column.name));
  return <section className="wizard-step">
    <header><h1 ref={headingRef} tabIndex={-1}>确认并运行</h1></header>
    <label className="wizard-name">任务名<input value={taskName(draft)} onChange={(event) => change({ type: "task-name", name: event.target.value })} /></label>
    <dl className="wizard-confirm-grid">
      <div><dt>源端</dt><dd>{confirmView.sourceLabel}</dd></div>
      <div><dt>目标表</dt><dd>{confirmView.targetTable}</dd></div>
      <div><dt>WHERE</dt><dd>{confirmView.where}</dd></div>
      <div><dt>主键</dt><dd>{confirmView.primaryKey.join(", ")}</dd></div>
      <div className="is-wide"><dt>字段映射</dt><dd>{confirmView.mappings.map((mapping) => <span className="mapping-chip" key={mapping.source}>{mapping.source} → {mapping.target}</span>)}</dd></div>
      <div><dt>写入方式</dt><dd>{WRITE_MODES.find((entry) => entry.mode === confirmView.write.mode)?.label ?? confirmView.write.mode}（{confirmView.write.statementLabel}）</dd></div>
      {/* agent 离线原来要等点了「开始导入」才知道——那是最贵的一次发现。 */}
      <div><dt>目标端 Agent</dt><dd><span className={draft.targetAgentOnline ? "confirm-check is-passed" : "confirm-check is-warn"}>{draft.targetAgentOnline ? "在线" : "离线"}</span></dd></div>
      <div className="is-wide"><dt>不搬的列</dt><dd>{dropped.length === 0 ? <span className="confirm-none">源表所有列都会搬</span> : dropped.map((column) => <span className="mapping-chip is-dropped" key={column.name}>{column.name}</span>)}</dd></div>
      <div className="is-wide"><dt>目标表检查</dt><dd><ConfirmTargetCheckCell check={confirmView.targetCheck} busy={busy === "check"} onCheck={loadCheck} /></dd></div>
    </dl>
    <Blockers blockers={model.blockers} />
    {/* 写入语义的常驻交底（2026-08 UX 评审 P0-1）：这一步是「开始导入」前的最后一屏，
        而这个产品**从不删行**。不写清楚的话，第一次用的人会按「全量同步」去理解
        自己刚配好的这张目标表。它不是告警，所以不着 --crit / --warn。
        文本按写法分叉（#261）：无主键那条路上「按主键 upsert」是句假话。 */}
    <UpsertNote text={confirmView.write.note} />
  </section>;
}

/**
 * 最后一屏上「目标表检查」那一格（2026-08 UX 评审 P0-3）。
 *
 * 三态，不是两态。「没检查过」原来和「检查通过」在这里长得一模一样——都写「已通过」，
 * 因为读的是空的 findings。而**恰恰是没检查过的那条路**（目标端 agent 离线，
 * `canAdvance` 明确放行）最需要在这里被说出来：它是草稿与生产写入之间的最后一句话。
 *
 * 没检查过时把原因摆出来，并给一颗**当场就能检查**的按钮——原因写在这里而补救要退回
 * 第 3 步，等于把人赶去找路。
 */
/**
 * 写入那一格（#261），摆在第 1 步映射表的正上方——也就是「主键」那一列旁边。
 *
 * 两行，**上面一行可选、下面一行推导**：
 *
 * - 「写入模式」是任务定义里的一个字段。今天只有一档，那也照样渲染成一组单选按钮而
 *   不是一行只读文字——清空后导入那一档接进来时，`WRITE_MODES` 多一项，这里什么都
 *   不用改；而一行只读文字要变成控件，改的是人对这一格的理解。
 * - 「写入语句」**不是选出来的**，人也改不了它：目标表有主键就 upsert，没有就纯
 *   追加。做成禁用的下拉框会让人以为它本来能选、只是此刻不让选。
 *
 * 下面那句 `note` 永远在，两种写法各说各的：需求方明确不要为无主键加勾选框，
 * 那这句话就是「重跑会产生重复数据」唯一的出口。
 */
function WriteModeCard({
  write,
  change,
}: {
  write: WriteView;
  change: (intent: Change) => void;
}) {
  return (
    <section className="write-mode-card">
      <header><div><strong>写入模式</strong><span>决定这次运行往目标表写什么</span></div></header>
      <div className="write-mode-body">
        <fieldset className="write-mode-choices">
          <legend className="visually-hidden">写入模式</legend>
          {write.modes.map((entry) => (
            <label key={entry.mode}>
              <input
                type="radio"
                name="write-mode"
                value={entry.mode}
                checked={write.mode === entry.mode}
                onChange={() => change({ type: "write-mode", mode: entry.mode })}
              />
              <span>{entry.label}</span>
              <small>{entry.hint}</small>
            </label>
          ))}
        </fieldset>
        <dl className="write-mode-statement">
          <dt>写入语句</dt>
          <dd>
            {write.statementLabel}
            <small>由目标表有没有主键决定，不由这里选</small>
          </dd>
        </dl>
      </div>
      <UpsertNote text={write.note} />
    </section>
  );
}

function ConfirmTargetCheckCell({
  check,
  busy,
  onCheck,
}: {
  check: ConfirmTargetCheck;
  busy: boolean;
  onCheck: () => void;
}) {
  if (check.state === "passed") {
    return <span className="confirm-check is-passed">已通过</span>;
  }
  if (check.state === "findings") {
    return <span className="confirm-check is-findings">有 {check.findings.length} 处需要处理</span>;
  }
  return <span className="confirm-check is-unchecked">
    <strong>尚未检查</strong>
    {check.excused !== null && <span className="confirm-check-reason">{check.excused}</span>}
    <button className="button" type="button" disabled={busy} onClick={onCheck}>
      {busy ? <LoaderCircle className="is-spinning" size={ICON.sm} /> : <RefreshCw size={ICON.sm} />}立即检查
    </button>
  </span>;
}

/**
 * 自定义 SQL 下左栏那一格：**这条 SQL 取出来的结果列**（UX 评审 P1-9）。
 *
 * 按表取数时这一格是表树，自定义 SQL 下原来什么都没有——而右边勾列、改名、点主键时
 * 反复要回头确认的，正好就是「这条 SQL 到底产出了哪些列、什么类型」。
 */
function ResultColumns({ columns, busy }: { columns: readonly BuilderColumn[]; busy: boolean }) {
  if (columns.length === 0) {
    return <p className="wizard-side-empty">{busy ? "正在识别结果列…" : "尚未识别结果列"}</p>;
  }
  return <div className="result-columns">
    <span className="result-columns-count">结果列 {columns.length}</span>
    <ul>
      {columns.map((column) => <li key={column.name}>
        <span className="mono">{column.name}</span>
        <small>{column.data_type}</small>
      </li>)}
    </ul>
  </div>;
}

function PreviewData({ preview }: { preview: PreviewResult }) {
  return <>
    <div className="table-wrap"><table className="data-grid preview-table"><thead><tr>{preview.columns.map((column) => <th key={column}>{column}</th>)}</tr></thead><tbody>{preview.rows.map((row, rowIndex) => <tr key={rowIndex}>{row.map((cell, columnIndex) => <td key={`${rowIndex}-${columnIndex}`}><span className="mono">{cell ?? "NULL"}</span></td>)}</tr>)}</tbody></table></div>
    <footer><span>{preview.rows.length} 条 · {preview.elapsed_ms} ms</span>{preview.truncated && <span>结果已截断，仅显示前 10 条</span>}</footer>
  </>;
}

/**
 * 映射步那块只读回显：SQL 前几行，长的截断——这里只是「认一眼」，不是读全文。
 *
 * 导出只为单测直接量它（`WizardConfirmDialog` 同一个路子）：这个项目没有 DOM harness，
 * 而截断这件事上唯一会错的地方——多截一行、少截一行、行数报错——只有拿真串子量才看得见。
 */
export function sqlEcho(sql: string): string {
  const trimmed = sql.trim();
  if (trimmed === "") return "（还没写）";
  const lines = trimmed.split("\n");
  return lines.length <= 6 ? trimmed : `${lines.slice(0, 6).join("\n")}\n…（共 ${lines.length} 行）`;
}

function previewIdentity(draft: Draft): string {
  return JSON.stringify({
    source_datasource_id: draft.source.datasource_id,
    spec: toSpec(draft),
  });
}

/**
 * 一步走不下去的理由，**分成两档**（UX 评审 P1-2）。
 *
 * 还没填的字段是「待办」：中性色、无标题、不进 live region——它们在向导刚打开、
 * 一步都还没走的时候就全部成立，用告警色说这件事等于开屏宣布出了三个问题。
 * 值与值打架、或者目标库不会接受的那些才是「错误」：红色，并且播报。
 *
 * 两档同时有的时候错误在上：那才是需要处理的，待办自己会随着填写消失。
 */
function Blockers({ blockers: allBlockers, label }: {
  blockers: ReturnType<typeof canAdvance>;
  label?: string;
}) {
  const blockers = allBlockers.filter((blocker) => blocker.column === null);
  const errors = blockers.filter((blocker) => blocker.kind === "error");
  const todos = blockers.filter((blocker) => blocker.kind === "todo");
  if (blockers.length === 0) return null;
  return <>
    {errors.length > 0 && <div className="wizard-mapping-problems" role="alert">
      {label !== undefined && <strong>{label}</strong>}
      <ul>{errors.map((blocker) => <li key={blocker.message}>{blocker.message}</li>)}</ul>
    </div>}
    {todos.length > 0 && <ul className="wizard-todos">
      {todos.map((blocker) => <li key={blocker.message}>{blocker.message}</li>)}
    </ul>}
  </>;
}

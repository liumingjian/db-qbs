/**
 * 登录页与改密对话框的**判定**，一处定义。屏幕只管画，不管判。
 *
 * 这套东西小到几乎不值得单开一个模块——单开它是因为「按钮什么时候按得动、
 * 为什么按不动」这类规则一旦长在 JSX 里，就会变成三处各写一遍、其中一处
 * 少一个条件。`wizard.ts` 与 `entry.ts` 已经是这个形状，这里跟着走。
 *
 * **口令强度不在这里判**，因为服务端也不判：出厂口令就是 `admin` 且长期有效，
 * 在它之上立一条「新口令至少八位」只会让改口令更烦，拦不住任何人。
 * 唯一的规矩是非空——空口令不是弱口令，是把登录表单变成纯装饰。
 */

export interface LoginForm {
  username: string;
  password: string;
}

export interface PasswordForm {
  current: string;
  next: string;
  confirm: string;
}

/**
 * 一次提交能不能发出去。挡住的**只是显然发不出去的那些**——
 * 口令对不对由服务端说了算，前端不预判，也没有本事预判。
 */
export type SubmitGate =
  | { kind: "ready" }
  | { kind: "blocked"; reason: string };

const READY: SubmitGate = { kind: "ready" };

function blocked(reason: string): SubmitGate {
  return { kind: "blocked", reason };
}

/**
 * 登录按钮的闸。
 *
 * **不提示「账号应当是 admin」**：这套东西只有一个账号，但把它写在登录页上
 * 等于替想试口令的人省掉第一步。表单该长得像个普通的登录表单。
 */
export function loginGate(form: LoginForm): SubmitGate {
  if (form.username.trim() === "") {
    return blocked("请输入账号");
  }
  if (form.password === "") {
    return blocked("请输入口令");
  }
  return READY;
}

/**
 * 改密按钮的闸。
 *
 * 顺序是刻意的：**先问当前口令，再问新口令，最后才问两次是否一致**。
 * 倒过来的话，一个把三栏都填错的人会先被告知「两次不一致」，改完再被告知
 * 「当前口令没填」——一次只说一件事，且说的是他接下来真要动的那一件。
 *
 * 「新口令与当前口令相同」是**拦下来而不是放行**：它不会报错，但它是一次
 * 什么也没发生的改密，而用户会以为自己改过了。
 */
export function passwordGate(form: PasswordForm): SubmitGate {
  if (form.current === "") {
    return blocked("请输入当前口令");
  }
  if (form.next === "") {
    return blocked("请输入新口令");
  }
  if (form.next === form.current) {
    return blocked("新口令与当前口令相同");
  }
  if (form.confirm !== form.next) {
    return blocked("两次输入的新口令不一致");
  }
  return READY;
}

export function emptyPasswordForm(): PasswordForm {
  return { current: "", next: "", confirm: "" };
}

/**
 * 闸的理由**什么时候该显示在屏幕上**。
 *
 * 一栏还空着就红着脸报「请输入当前口令」，是在骂一个还没开始打字的人。
 * 所以：闸关着、且三栏都动过之后，才把理由摆出来；在那之前按钮灰着就够了。
 */
export function gateReasonVisible(form: PasswordForm): boolean {
  return form.current !== "" && form.next !== "" && form.confirm !== "";
}

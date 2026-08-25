//! `db-qbs-source` 的进程入口。
//!
//! **这里刻意只剩十几行。** HTTP 路由、30 个 handler、发起运行的那套机器全都住在
//! lib 里（`db_qbs_source::http` / `db_qbs_source::server`），因为 `[[bin]]` 里的东西
//! `tests/` 一行都调不到——从前整条 dispatch 在这个文件里，测试只能 spawn 进程、
//! 手搓 socket，于是大半 handler 根本没被测过。

use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    match db_qbs_source::server::run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            db_qbs_source::server::report_startup_failure(&message);
            ExitCode::FAILURE
        }
    }
}

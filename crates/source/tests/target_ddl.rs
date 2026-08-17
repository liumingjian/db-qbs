use db_qbs_source::{generate_target_ddl, ColumnSupport, SourceColumn};

#[test]
fn target_ddl_is_derived_from_describe_columns() {
    let columns = vec![
        source_column("N_VA_PRICE", "NUMBER", Some(18), Some(4), None),
        source_column("C_NAME", "VARCHAR2", None, None, Some(50)),
        source_column("D_BIZ", "DATE", None, None, None),
    ];

    let ddl = generate_target_ddl(&columns, "T_POSITION", "D_BIZ").unwrap();

    assert_eq!(
        ddl,
        concat!(
            "-- db-qbs 生成的目标表建表 SQL，请自行执行；产品不会替你建表。\n",
            "-- 下面这条索引不是可选项：切换事务的 DELETE 会锁住目标表当日范围，\n",
            "-- 业务日期列无索引时锁全表。\n",
            "CREATE TABLE `T_POSITION` (\n",
            "  `N_VA_PRICE` DECIMAL(18,4) NULL,\n",
            "  `C_NAME` VARCHAR(50) NULL,\n",
            "  `D_BIZ` DATETIME(0) NULL,\n",
            "  KEY `idx_d_biz` (`D_BIZ`)\n",
            ") DEFAULT CHARSET=utf8mb4;"
        )
    );
    assert!(!ddl.contains("ALTER TABLE"));
}

#[test]
fn empty_target_table_uses_the_visible_placeholder() {
    let columns = vec![source_column("D_BIZ", "DATE", None, None, None)];

    let ddl = generate_target_ddl(&columns, "", "D_BIZ").unwrap();

    assert!(ddl.contains("CREATE TABLE <目标表名> ("));
}

fn source_column(
    name: &str,
    data_type: &str,
    precision: Option<i64>,
    scale: Option<i64>,
    length: Option<u64>,
) -> SourceColumn {
    SourceColumn {
        name: name.to_owned(),
        data_type: data_type.to_owned(),
        precision,
        scale,
        length,
        fsp: None,
        support: Some(ColumnSupport::Ok),
    }
}

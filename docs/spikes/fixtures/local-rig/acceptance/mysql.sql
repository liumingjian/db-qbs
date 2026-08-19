USE qbs;
DROP TABLE IF EXISTS M1_NARROW;
DROP TABLE IF EXISTS M1_WIDE;

-- 写入模型是 upsert（ADR-0035 §1）：主键列必须 NOT NULL，目标表必须真有对应唯一约束，
-- 否则 `ON DUPLICATE KEY UPDATE` 不报错、写得进去、重跑就多一份行。其余列仍必须可空（ADR-0009）。
CREATE TABLE M1_NARROW (
  ROW_ID DECIMAL(8,0) NOT NULL,
  V_TEXT VARCHAR(200) NULL,
  D_BIZ DATETIME(0) NULL,
  PRIMARY KEY (ROW_ID)
) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;

DELIMITER //
DROP PROCEDURE IF EXISTS create_m1_wide//
CREATE PROCEDURE create_m1_wide()
BEGIN
  DECLARE i INT DEFAULT 1;
  SET @ddl = 'CREATE TABLE M1_WIDE (ROW_ID DECIMAL(8,0) NOT NULL, D_BIZ DATETIME(0) NULL';
  -- VARCHAR(96) not 48: two row-size ceilings box the width in. 68 utf8mb4 VARCHAR(48)
  -- columns max out at 192 bytes each, which InnoDB must keep inline (only columns whose
  -- max exceeds 255 bytes are off-page eligible) and 68*193 blows the 8126-byte InnoDB
  -- limit (ERROR 1118). Going all the way to VARCHAR(255) blows the other ceiling: 68*1022
  -- declared bytes exceeds the 65535-byte table row maximum. 96 chars = 384 bytes sits in
  -- the window: off-page eligible, ~26 KB declared total, and the precheck still holds
  -- (target length >= source 48, charset utf8mb4).
  WHILE i <= 68 DO
    SET @ddl = CONCAT(@ddl, ', C', LPAD(i, 2, '0'), ' VARCHAR(96) NULL');
    SET i = i + 1;
  END WHILE;
  SET @ddl = CONCAT(@ddl, ', PRIMARY KEY (ROW_ID)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4');
  PREPARE statement FROM @ddl;
  EXECUTE statement;
  DEALLOCATE PREPARE statement;
END//
DELIMITER ;
CALL create_m1_wide();
DROP PROCEDURE create_m1_wide;

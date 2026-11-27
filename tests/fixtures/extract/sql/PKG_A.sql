-- PKG_A.sql — issue #193 example: three procedures in one file.
-- Extracting root `a1` must trim a2/a3; `a4` is a second root sharing the b1
-- dependency to demonstrate closure union + single shared-dep output.
CREATE OR REPLACE PROCEDURE a1 AS
BEGIN
    v := b1(1);
END;
/

CREATE OR REPLACE PROCEDURE a2 AS
BEGIN
    NULL;
END;
/

CREATE OR REPLACE PROCEDURE a3 AS
BEGIN
    NULL;
END;
/

CREATE OR REPLACE PROCEDURE a4 AS
BEGIN
    PERFORM b1(7);
END;
/

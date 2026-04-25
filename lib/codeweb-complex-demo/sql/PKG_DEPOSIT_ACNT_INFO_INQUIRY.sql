

PKG_DEPOSIT_ACNT_INFO_INQUIRY.sql
=======================================================================================================
create or replace package pkg_deposit_acnt_info_inquiry is
 TYPE refcur IS REF CURSOR;
 PROCEDURE prc_acnt_info_list(p_i_user_id                in varchar2,
                             p_i_role_id                 in varchar2,
                             p_i_qrybeginpos             IN VARCHAR2,
                             p_i_qrynum                  IN VARCHAR2,
                             p_i_qry_acnt                IN VARCHAR2,
                             p_i_qry_bank_pset           IN VARCHAR2,
                             p_i_qry_sys_flag            IN VARCHAR2,
                             p_i_qry_vald_flag           IN VARCHAR2,
                             p_i_qry_asset_type          IN VARCHAR2,
                             p_i_qry_bank_name           IN VARCHAR2,
                             p_i_qry_area_code           IN VARCHAR2,
                             out_code                    OUT VARCHAR2,
                             out_msg                     OUT VARCHAR2,
                             total_num                   OUT VARCHAR2,
                             out_list                    OUT refcur);
 PROCEDURE prc_acnt_info_exp(p_i_user_id              in varchar2,
                                p_i_role_id           in varchar2,
                                p_i_qry_acnt          IN VARCHAR2,
                                p_i_qry_bank_pset     IN VARCHAR2,
                                p_i_qry_sys_flag      IN VARCHAR2,
                                p_i_qry_vald_flag     IN VARCHAR2,
                                p_i_qry_asset_type    IN VARCHAR2,
                                p_i_qry_bank_name     IN VARCHAR2,
                                p_i_qry_area_code     IN VARCHAR2,
                                out_code              out varchar2,
                                out_msg               out varchar2,
                                total_num             out varchar2,
                                out_list              out REFCUR);
end pkg_deposit_acnt_info_inquiry;
/
create or replace package body pkg_deposit_acnt_info_inquiry is
 PROCEDURE prc_acnt_info_list(p_i_user_id                in varchar2,
                             p_i_role_id                 in varchar2,
                             p_i_qrybeginpos             IN VARCHAR2,
                             p_i_qrynum                  IN VARCHAR2,
                             p_i_qry_acnt                IN VARCHAR2,
                             p_i_qry_bank_pset           IN VARCHAR2,
                             p_i_qry_sys_flag            IN VARCHAR2,
                             p_i_qry_vald_flag           IN VARCHAR2,
                             p_i_qry_asset_type          IN VARCHAR2,
                             p_i_qry_bank_name           IN VARCHAR2,
                             p_i_qry_area_code           IN VARCHAR2,
                             out_code                    OUT VARCHAR2,
                             out_msg                     OUT VARCHAR2,
                             total_num                   OUT VARCHAR2,
                             out_list                    OUT refcur) IS
 BEGIN
   out_code := 0;
   out_msg  := '��ȡ���ڴ���˻���Ϣ���ݳɹ�';
   select /*+ use_cplan*/ count(1)
   into total_num
   from (
SELECT t.client_acnt_id, t.sys_acnt_id, t.fund_code, t.accno, t.accname,
 t.accnamefund, t.belong_bank_code, t.coin_code, t.zone_code, t.brno,
 t.acnt_type, t.bank_name, t.bank_code, t.bank_cexc, t.bank_bic,
 t.sys_flag, t.cnt_flag, t.dept_code, t.dept_type,
 t.auth_area, t.asset_type, t.accname_eng, '8' AS sub_src_type,
 t.vald_flag, t.inure_begin_date, t.inure_end_date, t.parent_acnt_id, t.sysupdatetm,e.asset_acnt_id
FROM v_par_client_acnt_info_noflag t, v_acnt_check_base_rule e
WHERE e.client_acnt_id = t.client_acnt_id and t.if_inter_bank = '2'
) temp
   left join par_fund_info fi
    on temp.fund_code = fi.fund_code
   left join (select t.area_name,t.area_code
       from par_sys_area t
       where to_char(now(), 'yyyymmdd') between t.inure_begin_date and
            t.inure_end_date) sysarea
   on fi.area_code = sysarea.area_code
   WHERE temp.sub_src_type = '8'
    AND EXISTS (SELECT /*+ no_expand */ 1
                FROM MV_ACCOUNT_PRIV v
               WHERE v.account_code = temp.asset_acnt_id
                 AND v.user_id = p_i_user_id
                 AND v.role = p_i_role_id)
    and (p_i_qry_acnt is null or temp.accno = p_i_qry_acnt)
    and (p_i_qry_bank_pset is null or temp.accno = p_i_qry_bank_pset)
    and (p_i_qry_sys_flag is null or temp.sys_flag= p_i_qry_sys_flag)
    and (p_i_qry_vald_flag is null or temp.vald_flag = p_i_qry_vald_flag)
    and (p_i_qry_asset_type is null or temp.asset_type = p_i_qry_asset_type)
    and (p_i_qry_bank_name is null or temp.bank_name like '%' || p_i_qry_bank_name || '%')
    and (p_i_qry_area_code is null or sysarea.area_code = p_i_qry_area_code);
   if total_num <= 0 then
     return;
   end if;
   open out_list for
   select /*+ use_cplan*/ fund_name,
   area_name,
   accno,
   accname,
   balance,
   bank_name,
   asset_type,
   coin_code,
   coin_name,
   security_name,
   sys_flag,
   cnt_flag,
   vald_flag,
   operator_name,
   check_user_name,
   sys_acnt_id
   from (select fi.fund_name,
      (select t.area_name
         from par_sys_area t
        where fi.area_code = t.area_code
        and to_char(now(), 'yyyymmdd') between t.inure_begin_date and
              t.inure_end_date) as area_name,
      temp.ACCNO,
      temp.ACCNAME,
      CASE temp.SYS_FLAG
        when '1' then
         to_char((select t.balance
                   from DAT_CLR_ACNT_BALANCE t
                  where t.asset_acnt_id = temp.asset_acnt_id
                    AND t.data_date = to_char(sysdate - 1, 'yyyymmdd')))
        when '2' then
         '��ϵͳ���˻�'
        else
         ''
      END as balance, -- ���
      temp.bank_name, -- ����������
      (select t.kind_name
         from dic_all_kind t
        where t.operation_kind = 'asset_type'
          and t.kind_id = temp.asset_type) as asset_type, -- �ʲ�����
      temp.coin_code,
      (select t.coin_name
         from par_sys_coin t
        where t.coin_code = temp.coin_code) as coin_name,
      (SELECT b.market_name || '--' || a.main_stock_code || '--' ||
              a.stock_short_name
         FROM par_sys_securities a, par_sys_market b, par_sys_acnt_info t
        WHERE a.main_market_code = b.market_code
          AND a.security_id = t.security_id
          AND t.acnt_id = temp.sys_acnt_id
          AND to_char(now(), 'yyyymmdd') BETWEEN a.inure_begin_date AND
              a.inure_end_date) as security_name, -- ��ӦͶ��Ʒ����
      temp.sys_flag,
      temp.cnt_flag,
      temp.vald_flag,
      (select message_value
         from usermessage um,v_par_client_acnt_info_noflag i
        where i.operator = um.user_id
        and temp.sys_acnt_id = i.sys_acnt_id
        and um.message_id = '001') operator_name,
      (select message_value
         from usermessage um,v_par_client_acnt_info_noflag i
        where i.check_user = um.user_id
        and temp.sys_acnt_id = i.sys_acnt_id
        and um.message_id = '001') check_user_name,
      temp.sys_acnt_id,
      row_number() over(ORDER BY temp.sys_acnt_id) rn
    from (
SELECT t.client_acnt_id, t.sys_acnt_id, t.fund_code, t.accno, t.accname,
     t.accnamefund, t.belong_bank_code, t.coin_code, t.zone_code, t.brno,
     t.acnt_type, t.bank_name, t.bank_code, t.bank_cexc, t.bank_bic,
     t.sys_flag, t.cnt_flag, t.dept_code, t.dept_type,
     t.auth_area, t.asset_type, t.accname_eng, '8' AS sub_src_type,
     t.vald_flag, t.inure_begin_date, t.inure_end_date, t.parent_acnt_id, t.sysupdatetm,e.asset_acnt_id
   FROM v_par_client_acnt_info_noflag t, v_acnt_check_base_rule e
   WHERE e.client_acnt_id = t.client_acnt_id and t.if_inter_bank = '2'
) temp
    left join par_fund_info fi
    on temp.fund_code = fi.fund_code
    left join (select t.area_name,t.area_code
         from par_sys_area t
         where to_char(now(), 'yyyymmdd') between t.inure_begin_date and
              t.inure_end_date) sysarea
    on fi.area_code = sysarea.area_code
    WHERE temp.sub_src_type = '8'
    AND EXISTS (SELECT /*+ no_expand */ 1
                FROM MV_ACCOUNT_PRIV v
               WHERE v.account_code = temp.asset_acnt_id
                 AND v.user_id = p_i_user_id
                 AND v.role = p_i_role_id)
    and (p_i_qry_acnt is null or temp.accno = p_i_qry_acnt)
    and (p_i_qry_bank_pset is null or temp.accno = p_i_qry_bank_pset)
    and (p_i_qry_sys_flag is null or temp.sys_flag= p_i_qry_sys_flag)
    and (p_i_qry_vald_flag is null or temp.vald_flag = p_i_qry_vald_flag)
    and (p_i_qry_asset_type is null or temp.asset_type = p_i_qry_asset_type)
    and (p_i_qry_bank_name is null or temp.bank_name like '%' || p_i_qry_bank_name || '%')
    and (p_i_qry_area_code is null or sysarea.area_code = p_i_qry_area_code))
    where rn BETWEEN to_number(p_i_qrybeginpos) AND to_number(p_i_qrybeginpos) + to_number(p_i_qrynum) - 1
    ORDER BY rn;
 EXCEPTION
   WHEN OTHERS THEN
     out_code := 1;
     out_msg  := '��ѯѰָ�����' || SQLERRM;
     pack_log.log('pkg_deposit_acnt_info_inquiry.prc_acnt_info_list', -- �洢������
                out_code, -- ������
                out_msg || sqlerrm, -- ����
                '4',
                '',
                '');
     RETURN;
 END;
 PROCEDURE prc_acnt_info_exp(p_i_user_id              in varchar2,
                                p_i_role_id           in varchar2,
                                p_i_qry_acnt          IN VARCHAR2,
                                p_i_qry_bank_pset     IN VARCHAR2,
                                p_i_qry_sys_flag      IN VARCHAR2,
                                p_i_qry_vald_flag    IN VARCHAR2,
                                p_i_qry_asset_type    IN VARCHAR2,
                                p_i_qry_bank_name     IN VARCHAR2,
                                p_i_qry_area_code     IN VARCHAR2,
                                out_code              out varchar2,
                                out_msg               out varchar2,
                                total_num             out varchar2,
                                out_list              out REFCUR) is
 v_sql      varchar2(32767);
 v_colname  varchar2(200);
 v_maxnum   number := 10000;
 begin
   out_code := '0';
   out_msg      := '���ڴ���˻���Ϣ���ݵ����ɹ�';
   select count(1)
   into total_num
   from (
SELECT t.client_acnt_id, t.sys_acnt_id, t.fund_code, t.accno, t.accname,
     t.accnamefund, t.belong_bank_code, t.coin_code, t.zone_code, t.brno,
     t.acnt_type, t.bank_name, t.bank_code, t.bank_cexc, t.bank_bic,
     t.sys_flag, t.cnt_flag, t.dept_code, t.dept_type,
     t.auth_area, t.asset_type, t.accname_eng, '8' AS sub_src_type,
     t.vald_flag, t.inure_begin_date, t.inure_end_date, t.parent_acnt_id, t.sysupdatetm,e.asset_acnt_id
   FROM v_par_client_acnt_info_noflag t, v_acnt_check_base_rule e
   WHERE e.client_acnt_id = t.client_acnt_id and t.if_inter_bank = '2'
) temp
   left join par_fund_info fi
    on temp.fund_code = fi.fund_code
   left join (select t.area_name,t.area_code
       from par_sys_area t
       where to_char(now(), 'yyyymmdd') between t.inure_begin_date and
            t.inure_end_date) sysarea
   on fi.area_code = sysarea.area_code
   WHERE temp.sub_src_type = '8'
    AND EXISTS (SELECT /*+ no_expand */ 1
                FROM MV_ACCOUNT_PRIV v
               WHERE v.account_code = temp.asset_acnt_id
                 AND v.user_id = p_i_user_id
                 AND v.role = p_i_role_id)
    and (p_i_qry_acnt is null or temp.accno = p_i_qry_acnt)
    and (p_i_qry_bank_pset is null or temp.accno = p_i_qry_bank_pset)
    and (p_i_qry_sys_flag is null or temp.sys_flag= p_i_qry_sys_flag)
    and (p_i_qry_vald_flag is null or temp.vald_flag = p_i_qry_vald_flag)
    and (p_i_qry_asset_type is null or temp.asset_type = p_i_qry_asset_type)
    and (p_i_qry_bank_name is null or temp.bank_name like '%' || p_i_qry_bank_name || '%')
    and (p_i_qry_area_code is null or sysarea.area_code = p_i_qry_area_code);
   if to_number(total_num) <= v_maxnum then
     open out_list for
     select fi.fund_name,
        (select t.area_name
           from par_sys_area t
          where fi.area_code = t.area_code
          and to_char(now(), 'yyyymmdd') between t.inure_begin_date and
                t.inure_end_date) as area_name,
        temp.ACCNO,
        temp.ACCNAME,
        CASE temp.SYS_FLAG
          when '1' then
           to_char((select t.balance
                   from DAT_CLR_ACNT_BALANCE t
                  where t.asset_acnt_id = temp.asset_acnt_id
                    AND t.data_date = to_char(sysdate - 1, 'yyyymmdd')))
          when '2' then
           '��ϵͳ���˻�'
          else
           ''
        END as balance, -- ���
        temp.bank_name, -- ����������
        (select t.kind_name
           from dic_all_kind t
          where t.operation_kind = 'asset_type'
            and t.kind_id = temp.asset_type) as asset_type, -- �ʲ�����
        (select t.coin_name
           from par_sys_coin t
          where t.coin_code = temp.coin_code) as coin_name,
        (SELECT b.market_name || '--' || a.main_stock_code || '--' ||
              a.stock_short_name
         FROM par_sys_securities a, par_sys_market b, par_sys_acnt_info t
        WHERE a.main_market_code = b.market_code
          AND a.security_id = t.security_id
          AND t.acnt_id = temp.sys_acnt_id
          AND to_char(now(), 'yyyymmdd') BETWEEN a.inure_begin_date AND
              a.inure_end_date) as security_name, -- ��ӦͶ��Ʒ����
        decode(temp.sys_flag,'1','ϵͳ��','2','ϵͳ��'),
        decode(temp.cnt_flag,'1','����','2','����'),
        decode(temp.vald_flag,'0','��Ч','1','��Ч'),
        (select message_value
         from usermessage um,v_par_client_acnt_info_noflag i
        where i.operator = um.user_id
        and temp.sys_acnt_id = i.sys_acnt_id
        and um.message_id = '001') operator_name,
      (select message_value
         from usermessage um,v_par_client_acnt_info_noflag i
        where i.check_user = um.user_id
        and temp.sys_acnt_id = i.sys_acnt_id
        and um.message_id = '001') check_user_name,
        temp.sys_acnt_id
      from (
  SELECT t.client_acnt_id, t.sys_acnt_id, t.fund_code, t.accno, t.accname,
     t.accnamefund, t.belong_bank_code, t.coin_code, t.zone_code, t.brno,
     t.acnt_type, t.bank_name, t.bank_code, t.bank_cexc, t.bank_bic,
     t.sys_flag, t.cnt_flag, t.dept_code, t.dept_type,
     t.auth_area, t.asset_type, t.accname_eng, '8' AS sub_src_type,
     t.vald_flag, t.inure_begin_date, t.inure_end_date, t.parent_acnt_id, t.sysupdatetm,e.asset_acnt_id
   FROM v_par_client_acnt_info_noflag t, v_acnt_check_base_rule e
   WHERE e.client_acnt_id = t.client_acnt_id and t.if_inter_bank = '2'
  ) temp
      left join par_fund_info fi
      on temp.fund_code = fi.fund_code
      left join (select t.area_name,t.area_code
           from par_sys_area t
           where to_char(now(), 'yyyymmdd') between t.inure_begin_date and
                t.inure_end_date) sysarea
      on fi.area_code = sysarea.area_code
      WHERE temp.sub_src_type = '8'
      AND EXISTS (SELECT /*+ no_expand */ 1
                FROM MV_ACCOUNT_PRIV v
               WHERE v.account_code = temp.asset_acnt_id
                 AND v.user_id = p_i_user_id
                 AND v.role = p_i_role_id)
      and (p_i_qry_acnt is null or temp.accno = p_i_qry_acnt)
      and (p_i_qry_bank_pset is null or temp.accno = p_i_qry_bank_pset)
      and (p_i_qry_sys_flag is null or temp.sys_flag= p_i_qry_sys_flag)
      and (p_i_qry_vald_flag is null or temp.vald_flag = p_i_qry_vald_flag)
      and (p_i_qry_asset_type is null or temp.asset_type = p_i_qry_asset_type)
      and (p_i_qry_bank_name is null or temp.bank_name like '%' || p_i_qry_bank_name || '%')
      and (p_i_qry_area_code is null or sysarea.area_code = p_i_qry_area_code);
   else
     begin
       v_colname := '�������,Ӫ����,�˺�,�˻�����,�˻����,������,�ʲ�����,����,��ӦͶ��Ʒ����,ϵͳ�����־,�������־,��Ч��־,����Ա,����Ա,�˻�ID';
       v_sql     :=  'select fi.fund_name,' ||
                     '(select t.area_name' ||
                     'from par_sys_area t' ||
                     'where fi.area_code = t.area_code' ||
                     'and to_char(now(), ''yyyymmdd'') between t.inure_begin_date and' ||
                     '        t.inure_end_date) as area_name,' ||
                     'temp.ACCNO,' ||
                     'temp.ACCNAME,' ||
                     'CASE temp.SYS_FLAG' ||
                     'when ''1'' then' ||
                     'to_char((select t.balance' ||
                     '     from DAT_CLR_ACNT_BALANCE t, v_acnt_check_base_rule e' ||
                     'where t.asset_acnt_id = temp.asset_acnt_id' ||
                     'AND t.data_date = to_char(sysdate - 1, ''yyyymmdd'')))' ||
                     'when ''2'' then' ||
                     '''��ϵͳ���˻�''' ||
                     'else' ||
                     '''''' ||
                     'END as balance,' ||
                     'temp.bank_name,' ||
                     '(select t.kind_name' ||
                     'from dic_all_kind t' ||
                     'where t.operation_kind = ''asset_type''' ||
                     '    and t.kind_id = temp.asset_type) as asset_type,' ||
                     'temp.coin_code,' ||
                     '(select t.coin_name' ||
                     'from par_sys_coin t' ||
                     'where t.coin_code = temp.coin_code) as coin_name,' ||
                     '(SELECT b.market_name || ''--'' || a.main_stock_code || ''--'' || a.stock_short_name' ||
                     'FROM par_sys_securities a, par_sys_market b, par_sys_acnt_info t' ||
                     'WHERE a.main_market_code = b.market_code' ||
                     '    AND a.security_id = t.security_id' ||
                     '    AND t.acnt_id = temp.sys_acnt_id' ||
                     '    AND to_char(now(), ''yyyymmdd'') BETWEEN a.inure_begin_date AND' ||
                     '        a.inure_end_date) as security_name,' ||
                     'decode(temp.sys_flag,''1'',''ϵͳ��'',''2'',''ϵͳ��''),' ||
                     'decode(temp.cnt_flag,''1'',''����'',''2'',''����''),' ||
                     'decode(temp.vald_flag,''0'',''��Ч'',''1'',''��Ч''),' ||
                     '(select message_value' ||
                     'from usermessage um,v_par_client_acnt_info_noflag i' ||
                     'where i.operator = um.user_id' ||
                     'and temp.sys_acnt_id = i.sys_acnt_id' ||
                     'and um.message_id = ''001'') operator_name,' ||
                     '(select message_value' ||
                     'from usermessage um,v_par_client_acnt_info_noflag i' ||
                     'where i.check_user = um.user_id' ||
                     'and temp.sys_acnt_id = i.sys_acnt_id' ||
                     'and um.message_id = ''001'') check_user_name,' ||
                     'temp.sys_acnt_id' ||
                     ' from (SELECT t.client_acnt_id, t.sys_acnt_id, t.fund_code, t.accno, t.accname, ' ||
                     ' t.accnamefund, t.belong_bank_code, t.coin_code, t.zone_code, t.brno, ' ||
                     ' t.acnt_type, t.bank_name, t.bank_code, t.bank_cexc, t.bank_bic, ' ||
                     ' t.sys_flag, t.cnt_flag, t.dept_code, t.dept_type, ' ||
                     ' t.auth_area, t.asset_type, t.accname_eng, ''8'' AS sub_src_type, ' ||
                     ' t.vald_flag, t.inure_begin_date, t.inure_end_date, t.parent_acnt_id, t.sysupdatetm,e.asset_acnt_id ' ||
                     ' FROM v_par_client_acnt_info_noflag t, v_acnt_check_base_rule e  ' ||
                     ' WHERE e.client_acnt_id = t.client_acnt_id and t.if_inter_bank = ''2'' ) temp ' ||
                     'left join par_fund_info fi' ||
                     'on temp.fund_code = fi.fund_code' ||
                     'left join (select t.area_name,t.area_code' ||
                     '    from par_sys_area t' ||
                     '    where to_char(now(), ''yyyymmdd'') between t.inure_begin_date and' ||
                     '            t.inure_end_date) sysarea' ||
                     'on fi.area_code = sysarea.area_code' ||
                     'WHERE temp.sub_src_type = ''8''' ||
                     'AND EXISTS (SELECT /*+ no_expand */ 1 FROM MV_ACCOUNT_PRIV v' ||
                     '    WHERE v.account_code = temp.asset_acnt_id' ||
                     'AND v.user_id = ''' || p_i_user_id || '''' ||
                     'AND v.role = ''' || p_i_role_id || ''')' ||
                     'and (' || p_i_qry_acnt || ' is null or temp.accno = ' || p_i_qry_acnt ||  ')' ||
                     'and (' || p_i_qry_bank_pset || ' is null or temp.accno = ' || p_i_qry_bank_pset || ')' ||
                     'and (' || p_i_qry_sys_flag || ' is null or temp.sys_flag=' || p_i_qry_sys_flag || ')' ||
                     'and (' || p_i_qry_vald_flag || ' is null or temp.vald_flag =' || p_i_qry_vald_flag || ')' ||
                     'and (' || p_i_qry_asset_type || 'is null or temp.asset_type =' || p_i_qry_asset_type || ')' ||
                     'and (' || p_i_qry_bank_name || ' is null or temp.bank_name like ''%'' || ' || p_i_qry_bank_name || '|| ''%'')' ||
                     'and (' || p_i_qry_area_code || ' is null or sysarea.area_code =' || p_i_qry_area_code || ')';
       pkg_rpt_batch_download.export_info_add('00000',
                                              '���ڴ���˻���Ϣ����',
                                              p_i_user_id,
                                              '',
                                              to_clob(v_sql),
                                              v_colname,
                                              out_code,
                                              out_msg);
       out_code := '1';
       out_msg      := '��¼��������1���������������첽��������.';
     EXCEPTION
       WHEN OTHERS THEN
         out_code := '1';
         out_msg      := '���ڴ���˻���Ϣ���ݵ�����¼��������1�������������첽��������ʧ��!' ||
                         sqlerrm;
         pack_log.log('pkg_deposit_acnt_info_inquiry.prc_acnt_info_exp', -- �洢������
                out_code, -- ������
                out_msg || sqlerrm, -- ����
                '4',
                '',
                '');
         RETURN;
     end;
   end if;
 exception
   when OTHERS then
     out_code := '1';
     out_msg      := 'ָ����������ҳ�������б���ѯʧ��' || sqlerrm;
     pack_log.log('pkg_deposit_acnt_info_inquiry.prc_acnt_info_exp', -- �洢������
                out_code, -- ������
                out_msg, -- ����
                '4',
                '',
                '');
   RETURN;
 end;
end pkg_deposit_acnt_info_inquiry;

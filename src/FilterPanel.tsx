import { useState, useMemo } from "react";
import { Checkbox, Input, Button, Text } from "@fluentui/react-components";
import { DismissRegular, SearchRegular, FilterRegular, DeleteRegular } from "@fluentui/react-icons";

// 团体 ID 就是口袋48 服务端用的 groupId（公演列表 / 按成员查录播都要它）
export const GROUPS: { name: string; id: number }[] = [
  { name: "SNH48", id: 10 },
  { name: "BEJ48", id: 11 },
  { name: "GNZ48", id: 12 },
  { name: "CKG48", id: 14 },
  { name: "IDFT", id: 15 },
  { name: "海外练习生", id: 16 },
  { name: "THE9", id: 17 },
  { name: "硬糖少女303", id: 18 },
  { name: "明星殿堂", id: 19 },
  { name: "丝芭影视", id: 20 },
  { name: "CGT48", id: 21 },
];

// 只有带团名的分组能用来按昵称前缀筛选成员直播
export const TEAM_GROUPS = GROUPS.filter((g) => /48$/.test(g.name));
const TYPES = ["视频", "电台", "连麦", "录屏", "游戏", "AI"];

interface FilterPreset {
  name: string;
  groups: string[];
  types: string[];
  members: string[];
  dateStart: string;
  dateEnd: string;
}

interface MemberInfo {
  nickname: string;
  userId?: string;
}

interface FilterPanelProps {
  open: boolean;
  onClose: () => void;
  activeTab: "live" | "record" | "open";
  // Current filter state
  filterGroups: string[];
  filterTypes: string[];
  filterMembers: string[];
  filterDateStart: string;
  filterDateEnd: string;
  // Setters
  onGroupsChange: (v: string[]) => void;
  onTypesChange: (v: string[]) => void;
  onMembersChange: (v: string[]) => void;
  onDateStartChange: (v: string) => void;
  onDateEndChange: (v: string) => void;
  // Data for member list
  allMembers: MemberInfo[];
  // 只看该成员录播（服务端 userId 查询）
  onlyMember: { nickname: string; userId: string } | null;
  onOnlyMemberChange: (value: { nickname: string; userId: string } | null) => void;
}

const STORAGE_KEY = "live48_filter_presets";

function loadPresets(): FilterPreset[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? JSON.parse(raw) : [];
  } catch {
    return [];
  }
}

function savePresetsToStorage(presets: FilterPreset[]) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(presets));
}

function FilterPanel({
  open,
  onClose,
  activeTab,
  filterGroups,
  filterTypes,
  filterMembers,
  filterDateStart,
  filterDateEnd,
  onGroupsChange,
  onTypesChange,
  onMembersChange,
  onDateStartChange,
  onDateEndChange,
  allMembers,
  onlyMember,
  onOnlyMemberChange,
}: FilterPanelProps) {
  const [memberSearch, setMemberSearch] = useState("");
  const [presetName, setPresetName] = useState("");
  const [presets, setPresets] = useState<FilterPreset[]>(loadPresets);

  const allGroupsSelected = filterGroups.length === 0;
  const allTypesSelected = filterTypes.length === 0;

  const filteredMembers = useMemo(() => {
    if (!memberSearch) return allMembers;
    const q = memberSearch.toLowerCase();
    return allMembers.filter(
      (m) => m.nickname.toLowerCase().includes(q) || (m.userId ?? "").includes(q)
    );
  }, [allMembers, memberSearch]);

  const toggleGroup = (group: string) => {
    if (filterGroups.includes(group)) {
      onGroupsChange(filterGroups.filter((g) => g !== group));
    } else {
      onGroupsChange([...filterGroups, group]);
    }
  };

  const toggleType = (type: string) => {
    if (filterTypes.includes(type)) {
      onTypesChange(filterTypes.filter((t) => t !== type));
    } else {
      onTypesChange([...filterTypes, type]);
    }
  };

  const toggleMember = (nick: string) => {
    if (filterMembers.includes(nick)) {
      onMembersChange(filterMembers.filter((m) => m !== nick));
    } else {
      onMembersChange([...filterMembers, nick]);
    }
  };

  const handleReset = () => {
    onGroupsChange([]);
    onTypesChange([]);
    onMembersChange([]);
    onDateStartChange("");
    onDateEndChange("");
    onOnlyMemberChange(null);
  };

  const handleSavePreset = () => {
    const name = presetName.trim();
    if (!name) return;
    const preset: FilterPreset = {
      name,
      groups: [...filterGroups],
      types: [...filterTypes],
      members: [...filterMembers],
      dateStart: filterDateStart,
      dateEnd: filterDateEnd,
    };
    const idx = presets.findIndex((p) => p.name === name);
    const next = idx >= 0 ? presets.map((p, i) => (i === idx ? preset : p)) : [...presets, preset];
    setPresets(next);
    savePresetsToStorage(next);
    setPresetName("");
  };

  const handleLoadPreset = (preset: FilterPreset) => {
    onGroupsChange([...preset.groups]);
    onTypesChange([...preset.types]);
    onMembersChange([...preset.members]);
    onDateStartChange(preset.dateStart);
    onDateEndChange(preset.dateEnd);
  };

  const handleDeletePreset = (name: string) => {
    const next = presets.filter((p) => p.name !== name);
    setPresets(next);
    savePresetsToStorage(next);
  };

  if (!open) return null;

  return (
    <>
      {/* Backdrop */}
      <div
        onClick={onClose}
        style={{
          position: "fixed",
          top: 0,
          left: 0,
          right: 0,
          bottom: 0,
          backgroundColor: "rgba(0,0,0,0.5)",
          zIndex: 5000,
          animation: "filter-fade-in 0.2s ease",
        }}
      />

      {/* Panel */}
      <div
        role="dialog"
        aria-label="高级筛选"
        style={{
          position: "fixed",
          top: 0,
          right: 0,
          bottom: 0,
          width: "min(380px, 85vw)",
          backgroundColor: "#1f1f1f",
          zIndex: 5001,
          display: "flex",
          flexDirection: "column",
          boxShadow: "-4px 0 24px rgba(0,0,0,0.4)",
          animation: "filter-slide-in 0.25s cubic-bezier(0.25, 0.8, 0.25, 1)",
          overflow: "hidden",
        }}
      >
        {/* Header */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "20px 20px 16px 20px",
            borderBottom: "1px solid rgba(255,255,255,0.08)",
            flexShrink: 0,
          }}
        >
          <div style={{ display: "flex", alignItems: "center", gap: "10px" }}>
            <FilterRegular style={{ fontSize: "22px", color: "#bb86fc" }} />
            <Text size={500} weight="semibold">高级筛选</Text>
          </div>
          <Button appearance="subtle" icon={<DismissRegular />} onClick={onClose} aria-label="关闭筛选" />
        </div>

        {/* Content */}
        <div
          style={{
            flex: 1,
            overflowY: "auto",
            padding: "20px",
            display: "flex",
            flexDirection: "column",
            gap: "24px",
          }}
        >
          {/* Groups */}
          <Section title="团体">
            <div style={{ display: "flex", flexWrap: "wrap", gap: "8px 16px" }}>
              {GROUPS.map((g) => (
                <Checkbox
                  key={g.id}
                  label={g.name}
                  checked={allGroupsSelected || filterGroups.includes(g.name)}
                  onChange={() => toggleGroup(g.name)}
                />
              ))}
            </div>
          </Section>

          {/* Types */}
          <Section title="类型">
            <div style={{ display: "flex", flexWrap: "wrap", gap: "8px 16px" }}>
              {TYPES.map((t) => (
                <Checkbox
                  key={t}
                  label={t}
                  checked={allTypesSelected || filterTypes.includes(t)}
                  onChange={() => toggleType(t)}
                />
              ))}
            </div>
          </Section>

          {/* Members */}
          <Section title="成员">
            <Input
              contentBefore={<SearchRegular />}
              placeholder="搜索成员..."
              value={memberSearch}
              onChange={(_, data) => setMemberSearch(data.value)}
              size="small"
              style={{ marginBottom: "8px" }}
            />
            <div
              style={{
                maxHeight: "200px",
                overflowY: "auto",
                display: "flex",
                flexDirection: "column",
                gap: "4px",
              }}
            >
              {filteredMembers.length === 0 ? (
                <Text size={200} style={{ color: "rgba(255,255,255,0.3)", padding: "8px 0" }}>
                  无匹配成员
                </Text>
              ) : (
                filteredMembers.map((member) => (
                  <div
                    key={member.nickname}
                    style={{ display: "flex", alignItems: "center", gap: "8px" }}
                  >
                    <Checkbox
                      label={member.nickname}
                      checked={filterMembers.length === 0 || filterMembers.includes(member.nickname)}
                      onChange={() => toggleMember(member.nickname)}
                      style={{ flex: 1, minWidth: 0 }}
                    />
                    {member.userId && (
                      <Button
                        appearance={onlyMember?.userId === member.userId ? "primary" : "subtle"}
                        size="small"
                        title="只看该成员录播"
                        onClick={() =>
                          onOnlyMemberChange(
                            onlyMember?.userId === member.userId
                              ? null
                              : { nickname: member.nickname, userId: member.userId as string }
                          )
                        }
                      >
                        只看TA录播
                      </Button>
                    )}
                  </div>
                ))
              )}
            </div>
            {filterMembers.length > 0 && (
              <Button
                appearance="subtle"
                size="small"
                onClick={() => onMembersChange([])}
                style={{ marginTop: "4px", alignSelf: "flex-start" }}
              >
                清除成员筛选
              </Button>
            )}
            {onlyMember && (
              <Button
                appearance="subtle"
                size="small"
                onClick={() => onOnlyMemberChange(null)}
                style={{ marginTop: "4px", alignSelf: "flex-start" }}
              >
                取消“只看 {onlyMember.nickname} 录播”
              </Button>
            )}
          </Section>

          {/* Date range (record tab only) */}
          {activeTab === "record" && (
            <Section title="日期范围">
              <div style={{ display: "flex", gap: "8px", alignItems: "center" }}>
                <input
                  type="date"
                  value={filterDateStart}
                  onChange={(e) => onDateStartChange(e.target.value)}
                  style={dateInputStyle}
                />
                <Text size={200} style={{ color: "rgba(255,255,255,0.5)", flexShrink: 0 }}>至</Text>
                <input
                  type="date"
                  value={filterDateEnd}
                  onChange={(e) => onDateEndChange(e.target.value)}
                  style={dateInputStyle}
                />
              </div>
              {(filterDateStart || filterDateEnd) && (
                <Button
                  appearance="subtle"
                  size="small"
                  onClick={() => { onDateStartChange(""); onDateEndChange(""); }}
                  style={{ marginTop: "4px", alignSelf: "flex-start" }}
                >
                  清除日期筛选
                </Button>
              )}
            </Section>
          )}

          {/* Save/Load presets */}
          <Section title="筛选方案">
            <div style={{ display: "flex", gap: "8px" }}>
              <Input
                placeholder="方案名称..."
                value={presetName}
                onChange={(_, data) => setPresetName(data.value)}
                size="small"
                style={{ flex: 1 }}
              />
              <Button
                appearance="primary"
                size="small"
                onClick={handleSavePreset}
                disabled={!presetName.trim()}
              >
                保存
              </Button>
            </div>
            {presets.length > 0 && (
              <div style={{ display: "flex", flexDirection: "column", gap: "4px", marginTop: "8px" }}>
                {presets.map((p) => (
                  <div
                    key={p.name}
                    style={{
                      display: "flex",
                      alignItems: "center",
                      gap: "8px",
                      padding: "6px 10px",
                      backgroundColor: "rgba(255,255,255,0.04)",
                      borderRadius: "6px",
                    }}
                  >
                    <Text
                      size={200}
                      style={{ flex: 1, cursor: "pointer", color: "rgba(255,255,255,0.8)" }}
                      onClick={() => handleLoadPreset(p)}
                    >
                      {p.name}
                    </Text>
                    <Button
                      appearance="subtle"
                      size="small"
                      icon={<DeleteRegular />}
                      onClick={() => handleDeletePreset(p.name)}
                      title="删除方案"
                    />
                  </div>
                ))}
              </div>
            )}
          </Section>
        </div>

        {/* Footer */}
        <div
          style={{
            padding: "16px 20px",
            borderTop: "1px solid rgba(255,255,255,0.08)",
            display: "flex",
            gap: "8px",
            flexShrink: 0,
          }}
        >
          <Button appearance="secondary" onClick={handleReset} style={{ flex: 1 }}>
            重置筛选
          </Button>
          <Button appearance="primary" onClick={onClose} style={{ flex: 1 }}>
            应用
          </Button>
        </div>
      </div>

      <style>{`
        @keyframes filter-fade-in {
          from { opacity: 0; }
          to { opacity: 1; }
        }
        @keyframes filter-slide-in {
          from { transform: translateX(100%); }
          to { transform: translateX(0); }
        }
      `}</style>
    </>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: "8px",
        padding: "16px",
        backgroundColor: "rgba(255,255,255,0.04)",
        borderRadius: "8px",
      }}
    >
      <Text size={300} weight="semibold" style={{ color: "rgba(255,255,255,0.7)" }}>
        {title}
      </Text>
      {children}
    </div>
  );
}

const dateInputStyle: React.CSSProperties = {
  flex: 1,
  backgroundColor: "rgba(255,255,255,0.08)",
  border: "1px solid rgba(255,255,255,0.15)",
  borderRadius: "4px",
  color: "white",
  padding: "4px 8px",
  fontSize: "13px",
  outline: "none",
  minHeight: "30px",
};

export default FilterPanel;

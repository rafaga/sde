-- ============================================================
-- Esquema corregido (SQLite, modo STRICT) — basado en EVE Online SDE
-- Requiere SQLite >= 3.37.0 (agosto 2021)
-- ============================================================
PRAGMA foreign_keys = ON;

-- ------------------------------------------------------------
-- Inventario
-- ------------------------------------------------------------

CREATE TABLE invCategories (
  categoryId    INTEGER NOT NULL PRIMARY KEY,
  categoryName  TEXT NOT NULL,
  published     INTEGER NOT NULL CHECK (published IN (0,1))
) STRICT;

CREATE TABLE invGroups (
  groupId     INTEGER NOT NULL PRIMARY KEY,
  groupName   TEXT NOT NULL,
  categoryId  INTEGER NOT NULL REFERENCES invCategories(categoryId)
                ON UPDATE CASCADE ON DELETE RESTRICT,
  anchorable  INTEGER NOT NULL CHECK (anchorable IN (0,1))
) STRICT;
CREATE INDEX idx_invGroups_categoryId ON invGroups(categoryId);

CREATE TABLE invTypes (
  typeId     INTEGER NOT NULL PRIMARY KEY,
  groupId    INTEGER REFERENCES invGroups(groupId)
               ON UPDATE CASCADE ON DELETE SET NULL,
  iconId     INTEGER,
  typeName   TEXT NOT NULL,
  published  INTEGER NOT NULL CHECK (published IN (0,1)),
  volume     REAL
) STRICT;
CREATE INDEX idx_invTypes_groupId ON invTypes(groupId);

-- ------------------------------------------------------------
-- Razas / NPCs / Facciones
-- ------------------------------------------------------------

CREATE TABLE races (
  raceId    INTEGER NOT NULL PRIMARY KEY,
  raceName  TEXT NOT NULL
) STRICT;

CREATE TABLE npcCorporations (
  corporationId               INTEGER NOT NULL PRIMARY KEY,
  corporationName             TEXT NOT NULL,
  tickerName                  TEXT NOT NULL,
  deleted                     INTEGER NOT NULL CHECK (deleted IN (0,1)),
  -- Verified against a real npcCorporations.jsonl sample (283
  -- records, August 2026): description present in 98.9%, extent/
  -- hasPlayerPersonnelManager/initialPrice/memberLimit/minSecurity/
  -- minimumJoinStanding/sendCharTerminationMessage/shares/size/
  -- taxRate/uniqueName in 100%.
  description                 TEXT,
  extent                      TEXT NOT NULL,
  hasPlayerPersonnelManager   INTEGER NOT NULL CHECK (hasPlayerPersonnelManager IN (0,1)),
  initialPrice                INTEGER NOT NULL,
  memberLimit                 INTEGER NOT NULL,
  minSecurity                 REAL NOT NULL,
  minimumJoinStanding         REAL NOT NULL,
  sendCharTerminationMessage  INTEGER NOT NULL CHECK (sendCharTerminationMessage IN (0,1)),
  shares                      INTEGER NOT NULL,
  size                        TEXT NOT NULL,
  -- Nullable: present in 66.8% of real records.
  sizeFactor                  REAL,
  taxRate                     REAL NOT NULL,
  uniqueName                  INTEGER NOT NULL CHECK (uniqueName IN (0,1)),
  -- ceoID/mainActivityID/secondaryActivityID reference entities this
  -- schema doesn't otherwise model (characters, activity types) -- kept
  -- as plain unconstrained integers rather than guessing at a FK
  -- target. Nullable per their real presence rate (ceoID 91.9%,
  -- mainActivityID 94.0%, secondaryActivityID only 12.0%).
  ceoId                        INTEGER,
  mainActivityId               INTEGER,
  secondaryActivityId          INTEGER,
  iconId                       INTEGER,
  raceId                       INTEGER REFERENCES races(raceId)
                                  ON UPDATE CASCADE ON DELETE SET NULL,
  -- enemyId/friendId are corporations disliking/allied with this one --
  -- self-referencing, and (like factionId/solarSystemId/stationId
  -- below) DEFERRABLE: the referenced corporation may not have been
  -- inserted yet when this row is (this table is parsed in file order,
  -- with no guarantee the referenced id comes first), or may belong to
  -- a phase that hasn't run yet. Deferring the check to COMMIT (inside
  -- parse_data()'s single transaction) resolves this the same way
  -- mapSystemGates.destinationGateId already does for its own mutual
  -- self-reference.
  enemyId                      INTEGER REFERENCES npcCorporations(corporationId)
                                  ON UPDATE CASCADE ON DELETE SET NULL
                                  DEFERRABLE INITIALLY DEFERRED,
  friendId                     INTEGER REFERENCES npcCorporations(corporationId)
                                  ON UPDATE CASCADE ON DELETE SET NULL
                                  DEFERRABLE INITIALLY DEFERRED,
  -- factionId: DEFERRABLE because `factions` is parsed *after*
  -- `npcCorporations` (factions.corporationId already depends on
  -- npcCorporations existing first) -- without deferring, a
  -- forward-reference to a not-yet-inserted faction would fail
  -- immediately in autocommit-equivalent terms.
  factionId                    INTEGER REFERENCES factions(factionId)
                                  ON UPDATE CASCADE ON DELETE SET NULL
                                  DEFERRABLE INITIALLY DEFERRED,
  -- solarSystemId/stationId: DEFERRABLE for the same reason --
  -- mapSolarSystems and npcStations are both
  -- parsed after npcCorporations.
  solarSystemId                INTEGER REFERENCES mapSolarSystems(solarSystemId)
                                  ON UPDATE CASCADE ON DELETE SET NULL
                                  DEFERRABLE INITIALLY DEFERRED,
  stationId                    INTEGER REFERENCES npcStations(stationId)
                                  ON UPDATE CASCADE ON DELETE SET NULL
                                  DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE INDEX idx_npcCorporations_raceId ON npcCorporations(raceId);
CREATE INDEX idx_npcCorporations_factionId ON npcCorporations(factionId);
CREATE INDEX idx_npcCorporations_solarSystemId ON npcCorporations(solarSystemId);
CREATE INDEX idx_npcCorporations_stationId ON npcCorporations(stationId);

-- Lookup of division *types* (R&D, Distribution, Mining, ...), from
-- npcCorporationDivisions.jsonl (10 records, confirmed complete for
-- _key/internalName/leaderTypeName/name; displayName in 9/10,
-- description in 5/10 -- neither modeled here, trimmed to what's
-- actually used elsewhere: which division a corporation has, not the
-- flavor text describing it).
CREATE TABLE npcCorporationDivisions (
  divisionId      INTEGER NOT NULL PRIMARY KEY,
  internalName    TEXT NOT NULL,
  leaderTypeName  TEXT NOT NULL
) STRICT;

-- Junction: which divisions a corporation actually has (its `divisions`
-- array, confirmed shape `{"_key": divisionId, "divisionNumber": int,
-- "leaderID": characterId, "size": int}`). `leaderId` is unconstrained
-- (no character table to reference, same as npcCorporations.ceoId
-- above) but NOT NULL: confirmed present in all 247 real entries across
-- every corporation's `divisions` array, unlike ceoId at the top level
-- (91.9%).
CREATE TABLE npcCorporationDivisionAssignments (
  corporationId   INTEGER NOT NULL REFERENCES npcCorporations(corporationId)
                     ON UPDATE CASCADE ON DELETE CASCADE,
  divisionId      INTEGER NOT NULL REFERENCES npcCorporationDivisions(divisionId)
                     ON UPDATE CASCADE ON DELETE CASCADE,
  divisionNumber  INTEGER NOT NULL,
  leaderId        INTEGER NOT NULL,
  size            INTEGER NOT NULL,
  CONSTRAINT pkey PRIMARY KEY (corporationId, divisionId) ON CONFLICT FAIL
) STRICT, WITHOUT ROWID;

-- Junction: which races may join this corporation (its
-- `allowedMemberRaces` array) -- same shape/purpose as `factionRace`,
-- just for corporations instead of factions.
CREATE TABLE npcCorporationAllowedRaces (
  corporationId  INTEGER NOT NULL REFERENCES npcCorporations(corporationId)
                    ON UPDATE CASCADE ON DELETE CASCADE,
  raceId         INTEGER NOT NULL REFERENCES races(raceId)
                    ON UPDATE CASCADE ON DELETE CASCADE,
  CONSTRAINT pkey PRIMARY KEY (corporationId, raceId) ON CONFLICT FAIL
) STRICT, WITHOUT ROWID;

-- Junction: which other corporations invest in this one, and by how
-- much (its `investors` array, `{"_key": corporationId, "_value":
-- shares}`) -- self-referencing and DEFERRABLE, same reasoning as
-- npcCorporations.enemyId/friendId above (the investing corporation may
-- not be inserted yet when this row is, since this table is parsed in
-- file order).
CREATE TABLE npcCorporationInvestors (
  corporationId  INTEGER NOT NULL REFERENCES npcCorporations(corporationId)
                    ON UPDATE CASCADE ON DELETE CASCADE,
  investorId     INTEGER NOT NULL REFERENCES npcCorporations(corporationId)
                    ON UPDATE CASCADE ON DELETE CASCADE
                    DEFERRABLE INITIALLY DEFERRED,
  shares         REAL NOT NULL,
  CONSTRAINT pkey PRIMARY KEY (corporationId, investorId) ON CONFLICT FAIL
) STRICT, WITHOUT ROWID;

-- Junction: per-item-type trade affinity (its `corporationTrades`
-- array, `{"_key": typeId, "_value": affinity}`). `_key` confirmed to
-- be a real `typeId` by cross-referencing every one of the 2705
-- distinct keys used across all 283 real corporations against a real
-- types.jsonl sample -- 100% matched, not a guess.
CREATE TABLE npcCorporationTrades (
  corporationId  INTEGER NOT NULL REFERENCES npcCorporations(corporationId)
                    ON UPDATE CASCADE ON DELETE CASCADE,
  typeId         INTEGER NOT NULL REFERENCES invTypes(typeId)
                    ON UPDATE CASCADE ON DELETE CASCADE,
  affinity       REAL NOT NULL,
  CONSTRAINT pkey PRIMARY KEY (corporationId, typeId) ON CONFLICT FAIL
) STRICT, WITHOUT ROWID;

CREATE TABLE factions (
  factionId             INTEGER NOT NULL PRIMARY KEY,
  factionName           TEXT NOT NULL,
  iconId                INTEGER NOT NULL,
  sizeFactor            REAL NOT NULL,
  uniqueName            INTEGER NOT NULL CHECK (uniqueName IN (0,1)),
  -- Verified against a real factions.jsonl sample (27 records, August
  -- 2026): description present in 100%, previously not captured at
  -- all. shortDescription/flatLogo/flatLogoWithName are much rarer
  -- (14.8%/66.7%/22.2%) but genuinely present in real data.
  description           TEXT NOT NULL,
  shortDescription      TEXT,
  flatLogo              TEXT,
  flatLogoWithName       TEXT,
  corporationId         INTEGER REFERENCES npcCorporations(corporationId)
                           ON UPDATE CASCADE ON DELETE SET NULL,
  -- The faction's militia corporation -- a second, distinct
  -- corporation from corporationId above. Present in 22.2% of real
  -- records.
  militiaCorporationId  INTEGER REFERENCES npcCorporations(corporationId)
                           ON UPDATE CASCADE ON DELETE SET NULL,
  -- DEFERRABLE: mapSolarSystems isn't parsed until after
  -- factions -- same reasoning as npcCorporations.solarSystemId.
  -- Present in 100% of real records (the faction's home system).
  solarSystemId          INTEGER REFERENCES mapSolarSystems(solarSystemId)
                           ON UPDATE CASCADE ON DELETE SET NULL
                           DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE INDEX idx_factions_corporationId ON factions(corporationId);
CREATE INDEX idx_factions_militiaCorporationId ON factions(militiaCorporationId);
CREATE INDEX idx_factions_solarSystemId ON factions(solarSystemId);

CREATE TABLE factionRace (
  factionId  INTEGER NOT NULL REFERENCES factions(factionId)
               ON UPDATE CASCADE ON DELETE CASCADE,
  raceId     INTEGER NOT NULL REFERENCES races(raceId)
               ON UPDATE CASCADE ON DELETE CASCADE,
  CONSTRAINT pkey PRIMARY KEY (factionId, raceId) ON CONFLICT FAIL
) STRICT, WITHOUT ROWID;

-- ------------------------------------------------------------
-- Mapa: regiones / constelaciones / sistemas
-- ------------------------------------------------------------

CREATE TABLE mapRegions (
  regionId    INTEGER NOT NULL PRIMARY KEY,
  regionName  TEXT NOT NULL,
  nebula      INTEGER NOT NULL,
  wormholeClassId INTEGER,
  factionId   INTEGER REFERENCES factions(factionId)
                ON UPDATE CASCADE ON DELETE SET NULL,
  centerX REAL NOT NULL, centerY REAL NOT NULL, centerZ REAL NOT NULL,
  maxProjX REAL NOT NULL DEFAULT(0.0), maxProjY REAL NOT NULL DEFAULT(0.0)
) STRICT;
CREATE INDEX idx_mapRegions_factionId ON mapRegions(factionId);

CREATE TABLE mapConstellations (
  constellationId  INTEGER NOT NULL PRIMARY KEY,
  constellationName TEXT NOT NULL,
  regionId  INTEGER NOT NULL REFERENCES mapRegions(regionId)
              ON UPDATE CASCADE ON DELETE RESTRICT,
  centerX REAL NOT NULL, centerY REAL NOT NULL, centerZ REAL NOT NULL
) STRICT;
CREATE INDEX idx_mapConstellations_regionId ON mapConstellations(regionId);

CREATE TABLE mapSolarSystems (
  solarSystemId   INTEGER NOT NULL PRIMARY KEY,
  solarSystemName TEXT NOT NULL,
  constellationId INTEGER REFERENCES mapConstellations(constellationId)
                    ON UPDATE CASCADE ON DELETE SET NULL,
  -- Confirmed mutually exclusive against a real 8490-record sample
  -- (August 2026): every record has at most one of hub/corridor/fringe
  -- true, never two at once (2715/1931/787 records respectively, plus
  -- 3057 with none) -- a single TEXT column, not three booleans.
  type          TEXT CHECK (type IN ('hub', 'corridor', 'fringe')),
  luminosity REAL,
  radius REAL NOT NULL,
  centerX REAL NOT NULL, centerY REAL NOT NULL, centerZ REAL NOT NULL,
  position2DX REAL, position2DY REAL,
  security REAL NOT NULL CHECK (security BETWEEN -1.0 AND 1.0),
  securityClass TEXT,
  -- Present in 692 of a real 8490-record sample (August 2026): mostly
  -- 8, plus scattered values 14-18 -- no small contiguous range like
  -- C1-C6=1-6 would suggest, so left unconstrained (no CHECK) rather
  -- than guessing at valid values. No lookup table exists for this in
  -- the SDE (unlike e.g. typeStar), so, same as
  -- mapRegions.wormholeClassId, it's a plain integer, not an FK.
  wormholeClassId INTEGER,
  -- Present in only 70 of a real 8490-record sample (August 2026).
  -- Not DEFERRABLE: `factions` is parsed before `mapSolarSystems`
  -- (unlike e.g. npcCorporations.factionId, which needs deferral),
  -- so this can be a plain, immediately-checked foreign key.
  factionId INTEGER REFERENCES factions(factionId)
              ON UPDATE CASCADE ON DELETE SET NULL
  -- `visualEffect` (a nebula/graphical-effect identifier string,
  -- present in only 130 of the 8490 records checked) is deliberately
  -- excluded: no gameplay purpose outside the client's own rendering,
  -- and no consumer of this crate has asked for it. Not modeled, and
  -- not counted as part of this table's write-side implementation.
) STRICT;
CREATE INDEX idx_mapSolarSystems_constellationId ON mapSolarSystems(constellationId);
CREATE INDEX idx_mapSolarSystems_factionId ON mapSolarSystems(factionId);

-- `border`/`regional`/`international` are NOT mutually exclusive, unlike
-- `hub`/`corridor`/`fringe` above: confirmed against the full real
-- 8490-record sample (August 2026) that 104 systems carry two or all
-- three simultaneously (72 border+hub+international+regional, 32
-- border+corridor+international+regional). A single column can't
-- represent that without silently dropping one of the values, so this
-- is a join table instead -- each applicable subType gets its own row.
CREATE TABLE mapSolarSystemSubType (
  solarSystemId INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
                  ON UPDATE CASCADE ON DELETE CASCADE,
  subType TEXT NOT NULL CHECK (subType IN ('border', 'regional', 'international')),
  CONSTRAINT pkey PRIMARY KEY (solarSystemId, subType) ON CONFLICT FAIL
) STRICT, WITHOUT ROWID;
CREATE INDEX idx_mapSolarSystemSubType_subType ON mapSolarSystemSubType (subType);

CREATE TABLE factionSolarSystem (
  solarSystemId INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
                  ON UPDATE CASCADE ON DELETE CASCADE,
  factionId     INTEGER NOT NULL REFERENCES factions(factionId)
                  ON UPDATE CASCADE ON DELETE CASCADE,
  CONSTRAINT pkey PRIMARY KEY (solarSystemId, factionId)
) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX factionId ON factionSolarSystem (factionId);

-- ------------------------------------------------------------
-- Disallowed anchorable categories/groups by Solar System
-- ------------------------------------------------------------
-- Two separate tables, not one: `disallowedAnchorCategories` and
-- `disallowedAnchorGroups` are independent arrays in the real SDE,
-- not a parent/child pair -- confirmed against the full real
-- dataset (August 2026): a system can restrict a specific group
-- (e.g. groupId 361, Mobile Warp Disruptor) without its category
-- (22, Deployable) appearing in its own disallowedAnchorCategories
-- at all (solar system 31000005 does exactly this), so modeling
-- them as one row with both a categoryId and a groupId would force
-- a pairing the source data doesn't have. Named `...Anchorable...`
-- (matching `invGroups.anchorable`, the real SDE attribute this is
-- about) rather than `...Anchor...`, and kept clearly distinct from
-- one another and from the table these replace.

-- Categories entirely disallowed from being anchored in a solar
-- system (e.g. categoryId 65 = Structure, 22 = Deployable).
CREATE TABLE mapSolarSystemDisallowedAnchorableCategories (
  solarSystemId INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
                  ON UPDATE CASCADE ON DELETE CASCADE,
  categoryId    INTEGER NOT NULL REFERENCES invCategories(categoryId)
                  ON UPDATE CASCADE ON DELETE CASCADE,
  CONSTRAINT pkey PRIMARY KEY (solarSystemId, categoryId) ON CONFLICT FAIL
) STRICT, WITHOUT ROWID;
CREATE INDEX idx_mapSolarSystemDisallowedAnchorableCategories_categoryId
  ON mapSolarSystemDisallowedAnchorableCategories (categoryId);

-- Specific groups disallowed from being anchored in a solar system
-- (e.g. groupId 361 = Mobile Warp Disruptor), independent of the
-- category-level restrictions above.
CREATE TABLE mapSolarSystemDisallowedAnchorableGroups (
  solarSystemId INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
                  ON UPDATE CASCADE ON DELETE CASCADE,
  groupId       INTEGER NOT NULL REFERENCES invGroups(groupId)
                  ON UPDATE CASCADE ON DELETE CASCADE,
  CONSTRAINT pkey PRIMARY KEY (solarSystemId, groupId) ON CONFLICT FAIL
) STRICT, WITHOUT ROWID;
CREATE INDEX idx_mapSolarSystemDisallowedAnchorableGroups_groupId
  ON mapSolarSystemDisallowedAnchorableGroups (groupId);


-- ------------------------------------------------------------
-- Portals / conections / celestial bodies
-- ------------------------------------------------------------

CREATE TABLE mapSystemGates (
  systemGateId  INTEGER NOT NULL,
  solarSystemId INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
                  ON UPDATE CASCADE ON DELETE RESTRICT,
  destinationGateId INTEGER NOT NULL REFERENCES mapSystemGates(systemGateId)
                       ON UPDATE CASCADE ON DELETE SET NULL
                       DEFERRABLE INITIALLY DEFERRED,
  destinationSystemId INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
                         ON UPDATE CASCADE ON DELETE RESTRICT,
  typeId INTEGER NOT NULL REFERENCES invTypes(typeId)
           ON UPDATE CASCADE ON DELETE RESTRICT,
  positionX REAL NOT NULL, positionY REAL NOT NULL, positionZ REAL NOT NULL,
  CONSTRAINT pkey PRIMARY KEY (systemGateId, solarSystemId) ON CONFLICT FAIL
) STRICT;
CREATE UNIQUE INDEX idx_mapSystemGates_systemGateId ON mapSystemGates(systemGateId);
CREATE INDEX idx_mapSystemGates_solarSystemId ON mapSystemGates(solarSystemId);
CREATE INDEX idx_mapSystemGates_typeId ON mapSystemGates(typeId);

CREATE TABLE mapSystemConnections (
  systemA INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
            ON UPDATE CASCADE ON DELETE RESTRICT,
  systemB INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
            ON UPDATE CASCADE ON DELETE RESTRICT,
  PRIMARY KEY (systemA, systemB),
  CHECK (systemA < systemB)
) STRICT;
CREATE INDEX idx_mapSystemConnections_systemA ON mapSystemConnections(systemA);
CREATE INDEX idx_mapSystemConnections_systemB ON mapSystemConnections(systemB);

CREATE TABLE mapPlanets (
  planetId INTEGER NOT NULL PRIMARY KEY,
  solarSystemId INTEGER REFERENCES mapSolarSystems(solarSystemId)
                  ON UPDATE CASCADE ON DELETE SET NULL,
  planetaryIndex INTEGER NOT NULL,
  fragmented INTEGER CHECK (fragmented IN (0,1)),
  radius REAL,
  locked INTEGER CHECK (locked IN (0,1)),
  typeId INTEGER NOT NULL REFERENCES invTypes(typeId)
           ON UPDATE CASCADE ON DELETE RESTRICT,
  positionX REAL NOT NULL, positionY REAL NOT NULL, positionZ REAL NOT NULL
) STRICT;
CREATE UNIQUE INDEX planetSystem ON mapPlanets (solarSystemId, planetaryIndex);
CREATE INDEX idx_mapPlanets_typeId ON mapPlanets(typeId);

CREATE TABLE typeStar (
  starTypeId INTEGER PRIMARY KEY,
  typeId INTEGER NOT NULL REFERENCES invTypes(typeId)
           ON UPDATE CASCADE ON DELETE CASCADE,
  name  TEXT NOT NULL CHECK (length(name) <= 4),
  color TEXT NOT NULL
) STRICT;
CREATE INDEX idx_typeStar_typeId ON typeStar(typeId);

CREATE TABLE mapStars (
  starId INTEGER NOT NULL PRIMARY KEY,
  solarSystemId INTEGER REFERENCES mapSolarSystems(solarSystemId)
                  ON UPDATE CASCADE ON DELETE RESTRICT,
  locked INTEGER CHECK (locked IN (0,1)),
  radius INTEGER,
  starTypeId INTEGER NOT NULL REFERENCES typeStar(starTypeId)
               ON UPDATE CASCADE ON DELETE CASCADE
) STRICT;
CREATE UNIQUE INDEX starId ON mapStars (solarSystemId, starId);
CREATE INDEX idx_mapStars_starTypeId ON mapStars(starTypeId);

CREATE TABLE mapMoons (
  moonId INTEGER NOT NULL,
  solarSystemId INTEGER REFERENCES mapSolarSystems(solarSystemId)
                  ON UPDATE CASCADE ON DELETE SET NULL,
  moonIndex INTEGER NOT NULL,
  planetId INTEGER REFERENCES mapPlanets(planetId)
             ON UPDATE CASCADE ON DELETE SET NULL,
  positionX REAL NOT NULL, positionY REAL NOT NULL, positionZ REAL NOT NULL,
  radius INTEGER,
  typeId INTEGER REFERENCES invTypes(typeId)
           ON UPDATE CASCADE ON DELETE SET NULL,
  CONSTRAINT pkey PRIMARY KEY (solarSystemId, moonId) ON CONFLICT FAIL
) STRICT;
CREATE UNIQUE INDEX moonId ON mapMoons(moonId);
CREATE INDEX idx_mapMoons_planetId ON mapMoons(planetId);

-- ------------------------------------------------------------
-- Stations / NPC corporations by system
-- ------------------------------------------------------------

-- Community-maintained bits stop here; the tables below cover NPC
-- station data, verified against real npcStations.jsonl (5210
-- records), stationOperations.jsonl (68 records), and
-- stationServices.jsonl (27 records), August 2026.
--
-- Replaces the old staStation/staCorporations declarations: neither is
-- in this schema at all. The real SDE export uses different table
-- names (npcStations, not staStation) and a materially different, richer
-- shape, so this isn't a rename -- it's a fresh design against the
-- actual data.

CREATE TABLE stationServices (
  serviceId    INTEGER NOT NULL PRIMARY KEY,
  serviceName  TEXT NOT NULL
) STRICT;

CREATE TABLE stationOperations (
  operationId          INTEGER NOT NULL PRIMARY KEY,
  activityId           INTEGER NOT NULL,
  operationName        TEXT NOT NULL,
  -- Nullable: present in 55/68 real records (80.9%).
  description          TEXT,
  -- Likelihood of this operation appearing in each map-zone type
  -- (border/corridor/fringe/hub of the *region*, unrelated to the
  -- identically-named mapSolarSystems columns, which describe a
  -- specific solar system's own zone membership, not an operation's
  -- affinity for a zone type).
  border               REAL NOT NULL,
  corridor             REAL NOT NULL,
  fringe               REAL NOT NULL,
  hub                  REAL NOT NULL,
  ratio                REAL NOT NULL,
  manufacturingFactor  REAL NOT NULL,
  researchFactor       REAL NOT NULL
) STRICT;

-- Junction: which services (real, 2 to 24 per operation) a station with
-- this operation type offers.
CREATE TABLE stationOperationServices (
  operationId  INTEGER NOT NULL REFERENCES stationOperations(operationId)
                 ON UPDATE CASCADE ON DELETE CASCADE,
  serviceId    INTEGER NOT NULL REFERENCES stationServices(serviceId)
                 ON UPDATE CASCADE ON DELETE CASCADE,
  CONSTRAINT pkey PRIMARY KEY (operationId, serviceId) ON CONFLICT FAIL
) STRICT, WITHOUT ROWID;

-- Junction: which station item type corresponds to this operation for
-- each station-size category. sizeKey is a bit-flag (1/2/4/8/16 in the
-- real data, confirmed against all 68 records) -- kept as a plain
-- INTEGER rather than decoded into named constants, since the SDE
-- itself doesn't document what each flag represents beyond the value.
-- Nullable at the parent level: present in 47/68 real records (69.1%).
CREATE TABLE stationOperationTypes (
  operationId  INTEGER NOT NULL REFERENCES stationOperations(operationId)
                 ON UPDATE CASCADE ON DELETE CASCADE,
  sizeKey      INTEGER NOT NULL,
  typeId       INTEGER NOT NULL REFERENCES invTypes(typeId)
                 ON UPDATE CASCADE ON DELETE CASCADE,
  CONSTRAINT pkey PRIMARY KEY (operationId, sizeKey) ON CONFLICT FAIL
) STRICT, WITHOUT ROWID;

CREATE TABLE npcStations (
  stationId                 INTEGER NOT NULL PRIMARY KEY,
  -- Nullable: missing in exactly 1 of 5210 real records (a special,
  -- singular station -- e.g. Thera-like -- whose orbitID also matches
  -- neither a real moon nor a real planet; see orbitMoonId/orbitPlanetId
  -- below).
  celestialIndex            INTEGER,
  operationId               INTEGER NOT NULL REFERENCES stationOperations(operationId)
                               ON UPDATE CASCADE ON DELETE RESTRICT,
  -- The real SDE's `orbitID` can be either a moon or a planet
  -- (confirmed against real data: 76.5% moons, 23.5% planets, ~0.02%
  -- neither) -- split into two mutually-exclusive nullable columns
  -- instead of one plain INTEGER, since SQL has no way to declare a
  -- single foreign key conditional on two different target tables.
  -- Exactly one of the two is non-NULL for the vast majority of
  -- stations; both are NULL for the rare exception.
  orbitMoonId               INTEGER REFERENCES mapMoons(moonId)
                               ON UPDATE CASCADE ON DELETE SET NULL,
  orbitPlanetId             INTEGER REFERENCES mapPlanets(planetId)
                               ON UPDATE CASCADE ON DELETE SET NULL,
  -- Nullable: present in 3986/5210 real records (76.5%) -- exactly the
  -- stations that orbit a moon (a station orbiting a planet directly
  -- has no "index" to report, same absence rate as orbitMoonId being
  -- NULL).
  orbitIndex                INTEGER,
  ownerId                   INTEGER NOT NULL REFERENCES npcCorporations(corporationId)
                               ON UPDATE CASCADE ON DELETE RESTRICT,
  positionX REAL NOT NULL, positionY REAL NOT NULL, positionZ REAL NOT NULL,
  reprocessingEfficiency    REAL NOT NULL,
  reprocessingHangarFlag    INTEGER NOT NULL,
  reprocessingStationsTake  REAL NOT NULL,
  solarSystemId             INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
                               ON UPDATE CASCADE ON DELETE RESTRICT,
  typeId                    INTEGER NOT NULL REFERENCES invTypes(typeId)
                               ON UPDATE CASCADE ON DELETE RESTRICT,
  useOperationName          INTEGER NOT NULL CHECK (useOperationName IN (0,1))
) STRICT;
CREATE INDEX idx_npcStations_solarSystemId ON npcStations(solarSystemId);
CREATE INDEX idx_npcStations_operationId ON npcStations(operationId);
CREATE INDEX idx_npcStations_ownerId ON npcStations(ownerId);

# rakkio

Language for rakkio's personal planning and shared groups. These definitions describe the product concepts, not which features are already implemented.

## Groups

**Group**:
A shared planning space, such as a college class, whose shared assignments and events appear in its members' agendas. Membership does not expose anyone's personal content.
_Avoid_: A copy of the leader's account

**Owner**:
The group's single accountable member who controls membership, leader appointments, ownership transfers and group deletion. The owner also has leader capabilities.

**Leader**:
A member allowed to manage shared assignments, events and invitations. A group can have multiple leaders.

**Member**:
A user who has joined a group. Ordinary members can view shared content and manage their own assignment progress, but cannot edit the shared content.

**Shared assignment**:
A group task with one shared definition and independent progress for each member. Completing it for one member does not complete it for anyone else.
_Avoid_: A personal-task copy, a submission, a grade

**Group event**:
An event explicitly published to a group, such as a lecture or exam. A leader's personal events do not become group events automatically.

**Personal progress**:
A member's own completion state for a shared assignment. It is private by default and can optionally be shared with that group's leaders.

**Handle**:
A user-chosen, case-insensitive, unique public identifier such as `@daniel`. It is distinct from a display name, email address and Discord username.

**Invitation**:
An offer to join a group, addressed to a user by handle or made available through a link or code. An invitation is not membership; the recipient must accept or confirm joining.

from copy import deepcopy
from typing import List
from attrs import define, field


@define
class Checkpoint:
    """Checkpoint that counts the tokens in messages between the start and end index"""

    start_index: int = field(default=0)  # inclusive
    end_index: int = field(default=0)  # inclusive
    token_count: int = field(default=0)

    def __deepcopy__(self, _) -> "Checkpoint":  # noqa: ANN001
        """
        Returns a deep copy of the Checkpoint object.
        """
        return Checkpoint(
            start_index=self.start_index,
            end_index=self.end_index,
            token_count=self.token_count,
        )


@define
class CheckpointData:
    """Aggregates all information about checkpoints"""

    # the total number of tokens in the exchange. this is updated every time a checkpoint is
    # added or removed
    total_token_count: int = field(default=0)

    # in order list of individual checkpoints in the exchange
    checkpoints: List[Checkpoint] = field(factory=list)

    # the offset to apply to the message index when calculating the last message index
    # this is useful because messages on the exchange behave like a queue, where you can only
    # pop from the left or right sides. This offset allows us to map the checkpoint indices
    # to the correct message index, even if we have popped messages from the left side of
    # the exchange in the past. we reset this offset to 0 when we empty the checkpoint data.
    message_index_offset: int = field(default=0)

    def __deepcopy__(self, memo: dict) -> "CheckpointData":
        """Returns a deep copy of the CheckpointData object."""
        return CheckpointData(
            total_token_count=self.total_token_count,
            checkpoints=deepcopy(self.checkpoints, memo),
            message_index_offset=self.message_index_offset,
        )

    @property
    def last_message_index(self) -> int:
        if not self.checkpoints:
            return -1  # we don't have enough information to know
        return self.checkpoints[-1].end_index - self.message_index_offset

    def reset(self) -> None:
        """Resets the checkpoint data to its initial state."""
        self.checkpoints = []
        self.message_index_offset = 0
        self.total_token_count = 0

    def pop(self, index: int = -1) -> Checkpoint:
        """Removes and returns the checkpoint at the given index."""
        popped_checkpoint = self.checkpoints.pop(index)
        self.total_token_count = self.total_token_count - popped_checkpoint.token_count
        return popped_checkpoint

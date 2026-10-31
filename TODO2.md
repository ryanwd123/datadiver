### Updates to "space" menu
I want to change the name of the space menu to be called Helper Menu, and the idea with that menu is that it provides you with the shortcut keys that are available to use on the data grid, as well as some additional shortcut keys that you can use quickly from the space menu that are not available from the data grid.

So can you update the space menu to call it Helper Menu? And also in the Helper Menu, can you have one section at the top that has the shortcut keys that are available on the data grid, and then that'll make it easy to learn those keys, even though the Helper Menu doesn't need to be visible for those keys to work.

Then in the bottom section of the Helper Menu, show the different commands that are available when the Helper Menu is showing. 

### Text editor features
- show helper menu with ctrl-k
    - Currently, Control-K brings up the Ask AI panel. We will change that to be corded. So when you press Ctrl K, then in the helper menu that shows up, we will put A with Ask AI. 
- Other items for SQL editor helper menu. 
    - c: move current select node to CTE
        - If there's already a CTE, this will add the current select statement to the bottom of the CTE and then add a new select statement that says just select * from the new CTE. And then let's make it so that it puts in a placeholder CTE name and then positions it with multiple cursors so that when you are naming the CTE, it's also updating the from statement below.
        - ![pasted-2026-10-31-075656.png](data/pasted-2026-10-31-075656.png)
    - t: Run the current select node. 
        - This will be used to be able to inspect the results of CTEs in subqueries within a larger query. When this gets run, it needs to be smart enough. Like, let's say this is the third CTE. To execute that, it needs to be smart enough to build a query that includes the prior CTEs that it may depend on. 
    - f: format sql
    
